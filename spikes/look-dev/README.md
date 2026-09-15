# Look-Dev-Spike: toon, stylized, realistic

Vergleichsbilder für die Entscheidung über den Rendering-Stil: dieselbe prozedurale Dark-Fantasy-Arena
in drei Shading-Looks und zwei Lastvarianten, offscreen auf dem CPU-Adapter gerendert. Eigenständiges
Crate mit eigenem `[workspace]`, damit Engine-Workspace, Lints und CI unberührt bleiben. Es nutzt
`grimoire_gpu` (Kontext, Offscreen-Ziel, Read-back) und bringt eigene minimale Mesh-, Licht-, Outline-,
Post- und Bullet-Shader mit, weil die Engine noch keine Mesh- und Lichtpipeline hat.

## Was verglichen wird

| Schlüssel | Shading | Trennung der Figuren vom Hintergrund |
|---|---|---|
| `toon` | Cel-Shading, 3 Bänder je Licht (0 / 0,45 / 1), kein Glanz | Screen-Space-Outlines aus Tiefe, Normale und Klasse |
| `stylized` | weiches Licht: Wrap-Diffus mit kühler Schattentönung, schwacher normierter Blinn-Phong-Glanz | Fresnel-Rimlight nur an Figuren, keine Outlines |
| `realistic` | Cook-Torrance (GGX, höhenkorreliertes Smith, Schlick), analytisches Ambient | nur Licht, kein Rim, keine Outlines |

Varianten: `calm` (24 Punktlichter, 180 feindliche Bullets, 12 eigene Bolts, Ritualkreis-Emission 0,35)
und `busy` (232 Punktlichter, 2 000 Bullets, 40 Bolts, Emission 0,6).

**Fairness.** Eine Szenenbeschreibung mit einem Seed, eine Kamera (65° Neigung, 35° FOV), eine
Materialtabelle und ein WGSL-Modul: Vertex-Shader, Lichtstruktur, Falloff, Ritualkreis-Decal,
Blob-Schatten und Emissives sind wörtlich geteilt. Die Looks unterscheiden sich nur im Fragment-Einstieg
(`fs_toon`, `fs_stylized`, `fs_realistic`) und im Outline-Pass von toon. Alle schreiben dieselben
Render-Targets (HDR-Farbe, Normale+Klasse, Tiefe) bei 2x SSAA und laufen durch denselben Post-Stack
(Box-Downsample, Bloom, Khronos PBR Neutral) ohne Look-Parameter.

**Kalibrierung.** Ein globaler Lichtintensitätsfaktor (für alle Looks gleich, `LIGHT_INTENSITY_FACTOR`)
und je Look eine Lichtverstärkung multiplizieren nur die aus Lichtern abgeleiteten Terme (Diffus,
Ambient, Glanz). Die Verstärkung wird so gewählt, dass der Boden-Median im calm-World-only-Bild 0,18
(sRGB-kodiert) trifft, und für busy unverändert übernommen. Der globale Faktor ist so gesetzt, dass
realistic eine Verstärkung von etwa 1,0 bekommt. Rimlight, Emissive-Meshes, Ritualkreis-Emission,
eigene Bolts und Bullets skaliert nichts; ihre HDR-Multiplikatoren behalten ihre entworfene Bedeutung.

Der feindliche Bullet-Pass (Crate-Vertrag §6: nur Palettenraum HOSTILE, eigener Glow, kein Tiefentest)
läuft nach dem Resolve und kennt keinen Look; drei byte-identische Bullets-only-Renders aus drei
Prozessen belegen das.

## Reproduzieren

Voraussetzungen: Rust 1.98, keine GPU. Gerendert wird nur auf dem CPU-Adapter (WARP unter Windows,
lavapipe unter Linux); das Binary verweigert andere Adapter.

```sh
spikes/look-dev/render_all.sh
```

Das Skript startet je Look x Variante einen kurzen Prozess (calm vor busy; busy übernimmt die
Lichtverstärkung aus dem calm-Laufprotokoll), danach je Zeitmessrunde einen Prozess (Standard 18 je
Variante, davon 3 verworfene Aufwärmrunden) und baut zum Schluss ohne GPU die Composites und
`metrics.md`.

| Variable | Bedeutung |
|---|---|
| `STEPS` | Teilmenge von `render,timing,compose`, kommagetrennt |
| `OUT` | Ausgabeverzeichnis (Standard `out`) |
| `TIMING_ROUNDS` | Zeitmessrunden je Variante einschließlich der 3 Aufwärmrunden |
| `LOOK_DEV_GUARD` | Kommando vor jedem GPU-Prozess; ein Exit ungleich 0 beendet das Skript mit Exit 3 |
| `RESUME=1` | vorhandene Läufe und Messrunden behalten und überspringen |

Einzelaufrufe, jeweils mit `GRIMOIRE_GPU_ADAPTER=software`:

```sh
cargo run --release -- --look stylized --variant busy --out out       # ein Look x Variante nach out/
cargo run --release -- --look toon --variant calm --out toon_calm.png  # nur der finale Frame
cargo run --release -- --time-round 0 --variant busy --out out         # eine Zeitmessrunde
cargo run --release -- --compose --out out                             # Composites und Messwerte, ohne GPU
```

Optional rendert `--sway` im busy-Lauf 16 Frames mit ±1,5° Gierschwenk bei 1x AA (zeitliches Flimmern),
und `--floor-space linear` die lineare Lesart des Kalibrierziels.

## Ausgabe (`out/`)

- `<look>_<variant>.png`: finaler Frame, 1280x720. `<look>_<variant>_world.png`: nach dem Resolve, vor
  Bullets und Marker; Grundlage der Kontrastmessung.
- `<look>_busy_1x_crop.png`: 640x360 **ohne** SSAA (Produktions-AA) um Spieler und nächsten Fackelkegel.
- `bullets_only_busy.png`: Bullet-Pass und Spielermarker auf transparentem Schwarz.
- `composite_side_by_side.png`: 1920x720, Spalten toon | stylized | realistic, calm oben, busy unten.
  `composite_crops.png`: 1920x360, busy in nativer Auflösung (x 320–960, y 300–660).
- `metrics.md`, `metrics.json`: Lichtfaktor und Verstärkungen, Bullet- und Figurenkontrast,
  Palettenraum-Zähler, Bullet-Hashes, relative Kosten, alle Look-Parameter, Befunde.
- `data/`: Laufprotokolle, Klassenmasken, Bullets-only je Look, Rohdaten der Zeitmessung. Eine optionale
  `data/notes.txt` (eine Zeile je Hinweis) übernimmt `--compose` als „Hinweise zu diesem Lauf“ in `metrics.md`.

PNGs und `data/` werden nicht versioniert, damit das Repository klein bleibt; `metrics.md` und
`metrics.json` schon.

## Auslegungen und Abweichungen von der Spezifikation

- **Kalibrierziel 0,18** gilt als sRGB-kodierte relative Luminanz des Bodens (linear 0,0273).
- **Kalibrierung als Lichtverstärkung** statt als Belichtungsskalar im Post-Stack: ein Belichtungsskalar
  hätte auch die gemeinsamen Emissives skaliert (Flammen in toon 1,65x heller, eigene Bolts weiß).
- **Toon-Bänder (Fehlerbehebung):** die spezifizierten Schwellen 0,02 / 0,20 (Mindestkantenbreite
  0,004) setzen voraus, dass `saturate(N·L)·atten` bis 1 reicht. Unter einer Säulenfackel in 3,8 Höhe
  erreicht der Boden höchstens etwa 0,061, das obere Band war also unerreichbar. Alle drei Werte sind
  mit demselben Faktor 0,2 skaliert: 0,004 / 0,04 / 0,0008.
- **calm-Spiralarme** mit 12 statt 20 Reiskörnern, damit calm die entworfenen 180 Bullets hat
  (36 + 36 + 90 + 18).
- **Umgestürzte Säule** bei (8,5, −4,0) statt (10, −6): an der Spezifikationsposition läuft sie durch
  den Sockel der intakten Säule bei 330°. Das Glutlicht darunter wandert mit.
- **Mauerbogen** bei (−7,45, 9,46) statt (−9, 8): der tangential zum Arenakreis liegende Bogen lief
  durch Säule und Sockel bei 150°. Er ist auf seinem eigenen Bogen (Radius 12,04) um 2,12 Einheiten
  verschoben, das Minimum für 1 Einheit Abstand zum Sockel.
- **Gebrochene Säulen** tragen ihre Fackel unter der Bruchkante statt in 3,6 Höhe.
- **busy:** Randkerzen, Runensteine und schwebende Glut bekommen sichtbare Emissive-Punkte.
- **Hintergrund:** die Clear-Farbe ist durch den Tonemapper zurückgerechnet, damit #06070A ankommt.
- **Toon-Bandkanten:** WGSL erlaubt nach dem Radius-Early-out der Lichtschleife keine Ableitungen;
  `fwidth(x)` wird je Licht analytisch aus `dpdx`/`dpdy` von Position und Normale nachgebildet. Das
  macht den Toon-Lichtterm im Spike teurer als entworfen.
- **Zeitmessung** in kurzen Prozessen je Runde statt in einem Prozess: je Prozess ein ungemessener
  Frame pro Look, dann alle drei Looks in je Runde rotierter Reihenfolge.
- **1x-Ausschnitte:** das Roberts-Kreuz der Outlines greift 2 px statt 3 SS-px weit.

## Grenzen

- Prozedurale Proxys ohne Texturen, Normal-Maps und Schatten benachteiligen realistic am meisten;
  einfache konvexe Proxys schmeicheln dem Rimlight (stylized) und den Outlines (toon).
- Kein Light-Clustering: jedes Fragment läuft über alle Lichter eines Uniform-Arrays (max. 256,
  `Features::empty()` und WebGL2-Downlevel-Limits). Das bläht absolute Kosten auf und staucht die Abstände.
- Zeiten stammen vom CPU-Adapter: nur relativ und **kein GPU-Budget**; CPU-Rasterisierung verzerrt auch
  relative Kosten.
- Standbilder und SSAA verbergen zeitliches Flimmern; dafür gibt es die 1x-Ausschnitte und `--sway`.

## Befunde (nicht getunt)

- **Fuß von PBR Neutral:** das Kalibrierziel liegt im Fuß des Tonemappers. Für einen Minimum-Kanal
  x < 0,08 zieht er `x − 6,25x²` von allen Kanälen ab; dunkle Töne werden gestaucht (Luminanz etwa
  quadratisch in der Verstärkung) und gesättigt, in allen Looks gleich.
- **Kontrastgrenze von Hexenmagenta:** H0 (L 0,266) erreicht 4,5:1 überhaupt nur vor Hintergründen mit
  L < 0,02. Magenta-Cluster-Lichter tönen den Boden unter dichten Bullet-Wolken zusätzlich zur
  Bullet-Farbe hin; die Lesbarkeit trägt der dunkle Rand.
