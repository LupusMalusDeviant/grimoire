# ADR-0001: winit 0.30 als Fenster- und Event-Schicht

- **Status:** Akzeptiert
- **Datum:** 2026-09-14
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo `docs/adr/0003-wgpu-als-gpu-schicht.md`, `docs/prd/0002-grimoire-engine-architektur.md`
  (FR-15, FR-16), `docs/prd/0013-input-system.md`, `docs/plans/0001-phase-p0-fundament.md` (WP1.5, WP3),
  [Crate-Verträge §5](../architektur/crate-vertraege.md#5-grimoire_platform)

## Kontext

`grimoire_platform` ist die unterste Engine-Schicht: natives Fenster, Betriebssystem-Event-Loop,
Roh-Input, Uhren und Dateisystem. Darüber erzeugt `grimoire_gpu` eine wgpu-Surface für das Fenster
(ADR-0003 im Spiel-Repo). Für Fenster und Events braucht die Engine eine Grundlage auf Windows,
macOS und Linux (X11 und Wayland), die später auch iOS und Android trägt. Wie bei der GPU-Schicht
gilt: „from scratch" braucht eine Untergrenze, und die Lernenergie soll in Engine-Architektur
fließen, nicht in dreifachen Plattform-Boilerplate.

## Anforderungen

- Fenster, Event-Loop und Tastatur-/Maus-Input auf Windows, macOS und Linux aus einer Codebasis.
- Rohe Fenster-Handles über `raw-window-handle` 0.6, damit wgpu ohne Zusatzschicht eine Surface anlegen kann.
- Physische, layoutunabhängige Tastencodes (WASD bleibt auf AZERTY an derselben Stelle, PRD-0013).
- HiDPI-Unterstützung (Skalierungsfaktor, physische Pixel).
- Reiner Rust-Build ohne C-Toolchain oder mitgelieferte native Bibliotheken (einfaches CI und Cross-Compiling).
- Tragfähiger Pfad zu Mobile (Suspend/Resume-Lebenszyklus).
- Keine Fremdtypen in der öffentlichen Engine-API; Austauschbarkeit hinter Traits.

## Optionen

1. **SDL2-Bindings (`sdl2`-Crate)** — ausgereift, sehr breite Plattform- und Gamepad-Abdeckung, Audio
   inklusive. Aber: C-Bibliothek muss gebaut oder mitgeliefert werden (CI, Cross-Compiling, macOS-
   Signatur), eigener Event-Loop-Stil neben Rust-Ownership, doppelte Zuständigkeit mit eigenem
   Audio-/Input-Code. SDL3 ist in Rust noch jung.
2. **GLFW (`glfw`-Crate)** — schlank und bewährt für Desktop-OpenGL/Vulkan. Aber: C-Abhängigkeit,
   kein Mobile, kein Web, Wayland-Unterstützung historisch hinterher; Fokus auf Kontext-Erzeugung,
   die wgpu nicht braucht.
3. **Eigene Implementierung je Betriebssystem** (Win32, Cocoa/AppKit, X11 + Wayland) — maximaler
   Lernwert und volle Kontrolle. Aber: vier bis fünf Plattform-Backends mit `unsafe`-FFI, Monate
   Arbeit vor dem ersten Spiel-Pixel, dauerhafte Wartung von HiDPI-, IME- und Fokus-Sonderfällen.
4. **winit 0.30 (gewählt)** — reines Rust, de-facto-Standard des wgpu-Ökosystems, liefert
   `raw-window-handle` 0.6 direkt, physische `KeyCode`s, Skalierungsfaktor, Windows/macOS/Linux
   (X11 und Wayland) sowie iOS/Android/Web. Seit 0.30 ein trait-basiertes `ApplicationHandler`-Modell.

## Entscheidung

Option 4. `grimoire_platform` nutzt `winit` 0.30 (gepinnt auf 0.30.13) für Fenster und Event-Loop.
`winit`-Typen verlassen `grimoire_platform` nie: Nach außen gibt es nur `PlatformWindow`,
`PlatformEvent`, `RawInputEvent`, `KeyCode`, `MouseButton` und `run_desktop`. Der Desktop-Runner
bildet winit-Events in einer reinen, getesteten Funktion auf Engine-Typen ab; eine
Headless-Implementierung (`run_headless`) erfüllt denselben `AppHandler`-Vertrag ohne winit.

## Konsequenzen

- (+) Ein Fenster-Code für alle Desktop-Plattformen, ohne C-Toolchain; wgpu nimmt das Fenster über
  `raw-window-handle` direkt entgegen.
- (+) Kapselung in `grimoire_platform`: Ein winit-Versionssprung (0.30 → 0.31 ändert die API erneut)
  oder ein späterer Wechsel auf eigene Backends betrifft nur diesen Crate, nicht Renderer oder Spiel.
- (+) Physische Tastencodes und HiDPI kommen fertig mit; `KeyCode` der Engine bleibt ein kleiner,
  stabiler Ausschnitt, unbekannte Tasten werden `Unidentified`.
- (−) **`ApplicationHandler`-Modell:** Der Event-Loop gehört winit und ruft zurück; `run_desktop`
  blockiert und muss auf dem Haupt-Thread laufen (macOS). Das Fenster darf erst in `resumed`
  entstehen, deshalb läuft `AppHandler::init` dort genau einmal. Die Engine-Schleife ist
  ereignisgetrieben (`frame` bei `RedrawRequested`, danach neuer Redraw-Wunsch) statt einer eigenen
  `loop`-Schleife; Fixed-Timestep-Akkumulation liegt oberhalb in `grimoire_sim`.
- (−) **Mobile später:** Auf iOS/Android kommen `suspended`/`resumed` mehrfach, und die Surface muss
  bei `suspended` freigegeben und bei `resumed` neu erzeugt werden. P0 erzeugt das Fenster genau
  einmal und ignoriert weitere `resumed`-Aufrufe; für Mobile braucht der `AppHandler`-Vertrag
  zusätzliche Suspend/Resume-Hooks (eigene Entscheidung, sobald ein Mobile-Ziel ansteht).
- (−) **Wayland-Taktung:** winit richtet `RedrawRequested` nur dann an den Frame-Callbacks des
  Compositors aus, wenn vor dem Präsentieren `Window::pre_present_notify` aufgerufen wird.
  `PlatformWindow` bietet dafür keinen Hook, und der Runner fordert den nächsten Redraw sofort an.
  Der Renderer muss deshalb mit VSync (FIFO) präsentieren, sonst dreht die Schleife ungebremst.
  Ein optionaler Hook ist Kandidat für eine spätere Vertragsrevision. Ohne gezeichneten Puffer
  blendet Wayland ein Fenster zudem gar nicht ein (betrifft das Beispiel `window`).
- (−) **Haupt-Thread:** `run_desktop` panikt außerhalb des Haupt-Threads (alle Desktop-Plattformen),
  der Event-Loop lässt sich pro Prozess nur einmal erzeugen, und auf macOS liefern die Roh-Handles
  nur auf dem Haupt-Thread einen Wert. GPU-Surfaces entstehen daher in den `AppHandler`-Callbacks.
- (−) Gamepads deckt winit nicht ab; dafür wird im Input-Stream (PRD-0013) eine eigene Lösung
  gewählt (z. B. `gilrs`), ebenfalls hinter `grimoire_platform`-Traits.
- (−) winit-Eigenheiten (z. B. Pixel- statt Zeilen-Scrolldeltas bei Touchpads) müssen im Runner
  normalisiert werden; der Umrechnungsfaktor ist im Code dokumentiert und getestet.
