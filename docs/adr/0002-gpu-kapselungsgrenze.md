# ADR-0002: GPU-Kapselungsgrenze am Render-Vertrag

- **Status:** Akzeptiert
- **Datum:** 2026-09-14
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo [ADR-0003 „wgpu als GPU-Schicht"](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/adr/0003-wgpu-als-gpu-schicht.md)
  (Spiel-Repo `fiends-n-patrons`, `docs/adr/0003-wgpu-als-gpu-schicht.md`),
  [Crate-Verträge §1 und §6](../architektur/crate-vertraege.md)

## Kontext

ADR-0003 des Spiel-Repos legt wgpu als GPU-Schicht fest und formuliert: „Kein Engine-Code
oberhalb von `grimoire_gpu` spricht wgpu-Typen direkt an." Wörtlich genommen dürfte auch
`grimoire_render` keine wgpu-Typen verwenden — `grimoire_gpu` müsste dann Pipelines, Puffer,
Bind-Groups, Render-Passes und Shader-Module vollständig in eigene Typen hüllen.

Beim Umsetzen von P0 (instanzierter Sprite-Pass, Surface-Verwaltung, Offscreen-Readback) zeigt
sich, dass eine solche Hülle fast jede wgpu-Struktur eins zu eins spiegeln würde. Gleichzeitig
bleibt das eigentliche Ziel der Formulierung gültig: Fassade (`grimoire`) und Spiele (`fnp_*`)
dürfen nie an wgpu gekoppelt sein, damit das GPU-Backend austauschbar bleibt und wgpu-Versions-
sprünge nicht bis ins Spiel durchschlagen.

Diese ADR präzisiert daher, **wo** die Grenze verläuft.

## Anforderungen

- Fassade und Spiele sehen ausschließlich datenorientierte Typen: `RenderFrame`,
  `SpriteInstance`, `Camera2D`, `RenderStats`, `RendererConfig`, `RenderError` und den
  objektsicheren `Renderer`-Trait.
- Ein wgpu-Upgrade (gepinnte Version, bewusste Commits) betrifft höchstens zwei Crates.
- Der Renderer soll WGSL, Pipelines und Puffer direkt und ohne Abstraktionsverlust nutzen können
  (Instancing, später Frame-Graph, Clustered Lighting, Post-FX).
- Keine Wochen an Wrapper-Code ohne sichtbaren Nutzen (Solo-Zeitbudget, „Sichtbares zuerst").
- Die Grenze muss mechanisch prüfbar sein (Abhängigkeitsgraph statt Code-Review-Disziplin).

## Optionen

1. **Vollständige dünne RHI in `grimoire_gpu`** — eigene Typen für Buffer, Texture, Pipeline,
   Bind-Group, Pass, Shader. `grimoire_render` kennt nur diese RHI.
   - (+) Wörtliche Erfüllung von ADR-0003.
   - (−) wgpu *ist* bereits eine RHI; die Hülle spiegelt deren API nahezu eins zu eins und
     verdoppelt die Pflege bei jedem wgpu-Upgrade.
   - (−) Jede neue Renderer-Technik erfordert zuerst RHI-Erweiterungen; hoher Aufwand, geringer Wert.
2. **wgpu überall sichtbar** — Fassade und Spiele dürfen wgpu-Typen verwenden (z. B. eigene Passes).
   - (+) Maximale Flexibilität, kein Vertragscode.
   - (−) Bricht die Austauschbarkeit; wgpu-Upgrades schlagen bis ins Spiel durch.
   - (−) Widerspricht dem datenorientierten Render-Vertrag (PRD-0002).
3. **Grenze am Render-Vertrag (gewählt)** — wgpu-Typen sind nur in `grimoire_gpu` und
   `grimoire_render` sichtbar; `grimoire_gpu` re-exportiert `wgpu` für `grimoire_render`.
   Oberhalb von `grimoire_render` existiert nur der datenorientierte `Renderer`-Vertrag.
   - (+) Austauschbarkeit bleibt dort erhalten, wo sie zählt: an der Schnittstelle zu Fassade und Spiel.
   - (+) Renderer-Code nutzt wgpu direkt; kein Spiegel-Code.
   - (−) Ein Backend-Wechsel ersetzt `grimoire_gpu` *und* die Implementierung in `grimoire_render`
     (nicht nur eine RHI-Schicht) — bewusst akzeptiert, da kein v1.0-Ziel.

## Entscheidung

Option 3. Die Aussage aus ADR-0003 „kein Engine-Code oberhalb von `grimoire_gpu` spricht
wgpu-Typen direkt an" wird präzisiert zu:

> **wgpu-Typen sind ausschließlich in `grimoire_gpu` und `grimoire_render` sichtbar.**
> `grimoire_gpu` besitzt Instanz, Adapter, Device, Queue, Surfaces und Offscreen-Ziele und
> re-exportiert `wgpu` (`grimoire_gpu::wgpu`) für `grimoire_render`. `grimoire_render` verwendet
> wgpu-Typen nur intern; seine **öffentliche API** enthält keine wgpu- und keine
> `grimoire_gpu`-Typen. Kein Crate oberhalb von `grimoire_render` hängt von `grimoire_gpu` oder
> `wgpu` ab.

Konkrete Regeln:

- `grimoire_render` deklariert `wgpu` nicht selbst als Abhängigkeit, sondern nutzt den
  Re-Export — so gibt es genau eine wgpu-Version im Workspace, gesteuert durch `grimoire_gpu`.
- Fehler aus `grimoire_gpu` (`GpuError`) werden in `grimoire_render` über eine private
  Abbildungsfunktion in `RenderError` übersetzt, nicht über ein öffentliches `From`-Impl
  (das würde `GpuError` in die öffentliche API tragen).
- Ob die Grenze eingehalten ist, zeigt der Abhängigkeitsgraph der *direkten* Abhängigkeiten:
  `cargo tree -i wgpu -e normal --depth 1` listet nur `grimoire_gpu`,
  `cargo tree -i grimoire_gpu -e normal --depth 1` nur `grimoire_render`. Transitiv hängt die
  Fassade selbstverständlich von wgpu ab; entscheidend ist, dass sie es nicht benennen kann.
- ADR-0003 im Spiel-Repo bleibt unverändert gültig; diese ADR verfeinert nur deren Grenzsatz.

## Konsequenzen

- (+) Fassade und Spiele bleiben vollständig wgpu-frei; `RenderFrame` ist reines Datenformat
  und in Headless-Läufen mit dem `NullRenderer` nutzbar.
- (+) wgpu-Upgrades (z. B. geänderte Surface-API wie `CurrentSurfaceTexture` in wgpu 30) bleiben
  auf zwei Crates begrenzt.
- (+) `grimoire_gpu` bleibt in P0 frei gestaltbar (Crate-Verträge §6), weil es nur einen Nutzer hat.
- (−) Ein natives Backend (Vulkan/Metal direkt) müsste später beide Crates neu schreiben; der
  stabile Vertrag dafür ist der `Renderer`-Trait, nicht eine RHI.
- (−) Die Regel „keine wgpu-Typen in der öffentlichen API von `grimoire_render`" ist nicht durch
  den Compiler erzwungen und muss im Review geprüft werden (Hinweis im Crate-Doc vorhanden).
