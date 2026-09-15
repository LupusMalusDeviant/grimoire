# Look-Dev-Spike: Messwerte

Erzeugt von `spikes/look-dev/render_all.sh`. Eine Szene, ein Seed, eine Kamera, eine Materialtabelle; die Looks unterscheiden sich nur im Fragment-Einstieg (`fs_toon`, `fs_stylized`, `fs_realistic`) und im Outline-Pass von toon.

Adapter: Microsoft Basic Render Driver (Cpu, Dx12) (Software-Adapter).

**Hinweise zu diesem Lauf:**

- Zeitmessung: in einem Teil der Runden (festgestellt zu Beginn von calm-Runde 1, 4 und 13–17 sowie busy-Runde 0–3 und 16–17) lief auf demselben Rechner ein weiterer Renderprozess. Die je Runde rotierte Reihenfolge und der Median dämpfen den Einfluss auf die relativen Werte, heben ihn aber nicht auf.

## Legende

- `toon`: Toon (Cel-Shading, 3 Lichtbänder, Outlines)
- `stylized`: Stilisiertes 3D (weiches Licht, Rimlight, ohne Outlines)
- `realistic`: Realistischer (GGX-Mikrofacetten, ohne Rim und Outlines)

`composite_side_by_side.png` (1920x720): Spalten von links nach rechts **toon | stylized | realistic**, Zeilen **calm oben, busy unten**; Zellen 640x360 (Frames linear halbiert), 4-px-Stege #000000 über den Zellkanten.

- `composite_crops.png` (1920x360): busy in nativer Auflösung (2x SSAA), Ausschnitt x 320–960, y 300–660; Spalten toon | stylized | realistic.
- `<look>_<variant>.png`: finaler Frame. `<look>_<variant>_world.png`: nach PostFxResolve, vor Bullets und Marker (Grundlage der Kontrastmessung).
- `<look>_busy_1x_crop.png`: 640x360, **ohne** SSAA gerendert (Produktions-AA), Ausschnitt zwischen Spieler und nächstem sichtbaren Fackelkegel: toon ab (285, 182); stylized ab (285, 182); realistic ab (285, 182);
- `bullets_only_busy.png`: Bullet-Pass und Marker auf transparentem Schwarz.

## Lichtkalibrierung

Kein Belichtungsskalar im Post-Stack (Belichtung 1,0 für alle). Stattdessen multipliziert der Shader jedes Looks die aus Lichtern abgeleiteten Terme (Punktlichter, Mond, Hemisphären-Ambient; Diffus und Glanz) mit **globalem Lichtintensitätsfaktor 8 × Lichtverstärkung des Looks**. Der Faktor ist für alle Looks gleich und so gesetzt, dass realistic eine Verstärkung von etwa 1,0 bekommt. Nicht skaliert werden Rimlight (stylized), Emissive-Meshes (Flammen, Augen, Orb), Ritualkreis-Emission, eigene Bolts und Bullets: ihre HDR-Multiplikatoren behalten die entworfene Bedeutung.

Die Verstärkung je Look ist so gewählt, dass der Median der Boden-Luminanz (Klasse 0, ohne Ritualkreis und Blob-Schatten) im calm-World-only-Bild 0.18 ±5 % trifft, als sRGB-kodierte relative Luminanz (Anzeigewert) (linear 0.0272); Abbruch bei 1 % Abweichung. busy nutzt dieselbe Verstärkung unverändert.

| Look | Lichtverstärkung | wirksame Lichtskala (Faktor × Verstärkung) | Boden-Median calm (linear) | Iterationen |
|---|---:|---:|---:|---:|
| toon | 0.406 | 3.245 | 0.0272 | 3 |
| stylized | 0.720 | 5.760 | 0.0272 | 3 |
| realistic | 1.000 | 8.000 | 0.0271 | 1 |

## Bullet-Kontrast

WCAG-Kontrast der Bullet-Körperfarbe gegen die mittlere Luminanz eines Rings 3–6 px außerhalb des projizierten Radius im World-only-Bild (kreisförmig um den großen Radius, auch bei Reis). Bullets ohne Ringpixel im Bild zählen nicht. Die Randfarbe #0A0510 steht zum Vergleich daneben. Obergrenzen: H0 Hexenmagenta (L = 0.266) erreicht 4,5:1 nur vor einem Hintergrund mit L < 0.020, H1 Giftlimette (L = 0.817) bis L < 0.143.

| Look | Variante | Bullets | min | 5. Perz. | Median | Anteil ≥ 4,5:1 | Median H0 Magenta | Median H1 Limette | Median Rand |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| toon | calm | 180 | 1.10 | 1.65 | 5.12 | 53 % | 2.57 | 6.72 | 2.42 |
| toon | busy | 1921 | 1.01 | 1.34 | 3.55 | 33 % | 2.20 | 5.08 | 2.98 |
| stylized | calm | 180 | 1.05 | 1.32 | 3.76 | 39 % | 2.50 | 5.90 | 2.68 |
| stylized | busy | 1921 | 1.00 | 1.10 | 2.54 | 21 % | 1.68 | 3.92 | 3.96 |
| realistic | calm | 180 | 1.05 | 1.40 | 3.76 | 41 % | 2.66 | 6.11 | 2.51 |
| realistic | busy | 1921 | 1.00 | 1.12 | 2.63 | 21 % | 1.73 | 4.10 | 3.81 |

## Figur-gegen-Hintergrund-Kontrast

Aus der gemeinsamen Klassenmaske: Ring 2–6 px außerhalb der Silhouette (ohne Pixel anderer Figuren) gegen (a) die mittlere Luminanz der beleuchteten Silhouette ohne Augen/Orb und (b) das innere Konturband 0–3 px. 9 Figuren.

| Look | Variante | Median gesamt | min gesamt | Median Kontur | min Kontur |
|---|---|---:|---:|---:|---:|
| toon | calm | 1.83 | 1.26 | 1.57 | 1.02 |
| toon | busy | 1.78 | 1.22 | 1.72 | 1.01 |
| stylized | calm | 2.26 | 1.03 | 1.68 | 1.03 |
| stylized | busy | 1.97 | 1.14 | 1.56 | 1.10 |
| realistic | calm | 1.53 | 1.18 | 1.49 | 1.05 |
| realistic | busy | 1.71 | 1.17 | 1.55 | 1.08 |

Je Figur (gesamt / Kontur), Reihenfolge: 1 Spieler, 2–7 Imps, 8–9 Brutes:

| Look | Variante | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
|---|---|---|---|---|---|---|---|---|---|---|
| toon | calm | 1.26 / 1.02 | 1.74 / 1.46 | 1.83 / 1.57 | 2.56 / 2.21 | 3.41 / 2.48 | 3.16 / 2.24 | 4.00 / 2.90 | 1.28 / 1.02 | 1.27 / 1.30 |
| toon | busy | 1.28 / 1.01 | 1.44 / 1.21 | 1.48 / 1.34 | 2.73 / 2.21 | 3.33 / 2.56 | 2.91 / 2.09 | 3.50 / 2.52 | 1.22 / 1.01 | 1.78 / 1.72 |
| stylized | calm | 1.03 / 1.03 | 1.27 / 1.23 | 1.54 / 1.45 | 2.82 / 2.58 | 3.01 / 2.62 | 4.63 / 3.89 | 4.98 / 4.04 | 2.26 / 1.68 | 1.03 / 1.12 |
| stylized | busy | 1.14 / 1.10 | 1.19 / 1.18 | 1.48 / 1.46 | 3.19 / 2.80 | 3.22 / 2.93 | 3.81 / 3.36 | 2.40 / 1.96 | 1.97 / 1.56 | 1.45 / 1.37 |
| realistic | calm | 1.18 / 1.16 | 1.25 / 1.24 | 1.53 / 1.49 | 2.52 / 2.22 | 2.84 / 2.44 | 3.95 / 3.24 | 4.38 / 3.57 | 1.37 / 1.05 | 1.40 / 1.43 |
| realistic | busy | 1.32 / 1.24 | 1.17 / 1.18 | 1.49 / 1.55 | 2.80 / 2.42 | 3.12 / 3.02 | 3.19 / 2.78 | 2.36 / 1.95 | 1.26 / 1.08 | 1.71 / 1.17 |

## Palettenraum und Bullet-Pass

| Look | Variante | Bullets gezeichnet | bullets_rejected_palette_space | bullets_rejected_invalid | Szenen-Lichter | Freundliche Bolts | Stress (Fackel/Kreis/Spieler/Säule) |
|---|---|---:|---:|---:|---:|---:|---|
| toon | calm | 180 | 0 | 0 | 24 | 12 | 6/6/3/3 |
| toon | busy | 2000 | 0 | 0 | 232 | 40 | 20/20/10/10 |
| stylized | calm | 180 | 0 | 0 | 24 | 12 | 6/6/3/3 |
| stylized | busy | 2000 | 0 | 0 | 232 | 40 | 20/20/10/10 |
| realistic | calm | 180 | 0 | 0 | 24 | 12 | 6/6/3/3 |
| realistic | busy | 2000 | 0 | 0 | 232 | 40 | 20/20/10/10 |

Punktlichter je Gruppe:

- calm: 24 = 8 Fackeln und Kohlebecken + 4 Ritual + 2 Altarkerzen + 1 Stab-Orb + 2 Brute-Augen + 6 Bullet-Cluster calm + 1 Glutspalt
- busy: 232 = 8 Fackeln und Kohlebecken + 4 Ritual + 2 Altarkerzen + 1 Stab-Orb + 2 Brute-Augen + 6 Bullet-Cluster calm + 1 Glutspalt + 64 Bullet-Cluster busy + 64 Randkerzen + 48 Runensteine + 32 schwebende Glut

Selbsttest: eine zusätzlich eingeschleuste FRIENDLY-Instanz wird verworfen (`bullets_rejected_palette_space` = 1).

Look-Unabhängigkeit: Bullet-Pass und Marker je Look in einem eigenen Prozess auf transparentes Schwarz gerendert (FNV-1a-64 der RGBA-Bytes):

- toon: `9eb47fa3a597a44e`
- stylized: `9eb47fa3a597a44e`
- realistic: `9eb47fa3a597a44e`

Byte-identisch: **ja**.

## Relative Kosten

**Software-Adapter (WARP), kein GPU-Budget.** CPU-Wandzeit um Queue-Submit plus `device.poll(Wait)` je Schritt, relativ zu toon derselben Variante. Jede Runde ist ein eigener kurzer Prozess: jeder Look wird zuerst einmal ungemessen gerendert, dann werden alle drei in je Runde rotierter Reihenfolge gemessen. 3 Aufwärmrunden verworfen, Median aus 15 Runden. Die CPU-Rasterisierung verzerrt auch relative Kosten (SIMD-freundliche Mathematik gegen Verzweigungen und `pow`), und die Schleife über alle Lichter ohne Clustering staucht die Abstände. Der Toon-Lichtterm ist im Spike schwerer als entworfen, weil WGSL nach dem Radius-Early-out keine Ableitungen erlaubt und `fwidth(x)` je Licht analytisch nachgebildet wird.

| Variante | Look | World-Pass | Outline-Pass (Anteil am toon-Frame) | Post + Bullets | Gesamt |
|---|---|---:|---:|---:|---:|
| calm | toon | 1.00 | 0.07 | 1.00 | 1.00 |
| calm | stylized | 1.00 | 0.00 | 1.01 | 0.93 |
| calm | realistic | 0.97 | 0.00 | 1.00 | 0.91 |
| busy | toon | 1.00 | 0.02 | 1.00 | 1.00 |
| busy | stylized | 0.89 | 0.00 | 1.00 | 0.88 |
| busy | realistic | 0.81 | 0.00 | 1.01 | 0.81 |

## Eingefrorene Look-Parameter

Gemeinsam: Albedo je Material (Tabelle unten), Licht-Falloff `saturate(1-(d/r)^4)^2/(1+d^2)`, Mond #9AB0D8 I 0,35 entlang (-0,35, 0,5, -0,8), Hemisphären-Ambient Himmel #1C2438 / Boden #110D0B I 0,35, Bloom Schwelle 1,0 / Knie 0,5 / 4 Stufen / 0,08, Khronos PBR Neutral, 2x SSAA.

- **toon:** Bänder je Punktlicht bei x = 0.004 und 0.04 (Stufen 0 / 0,45 / 1,0; x = saturate(N·L)·atten), Beitrag `c·I·0,3·B(x)`, Kantenbreite `max(fwidth(x), 0.0008)` (analytisch propagiert); Mond bei 0,05 und 0,5 mit `max(fwidth(x), 0,004)`; Outline: Roberts-Kreuz 3 SS-px (1x: 2 px), Tiefe 0,015 relativ, Normale 0,35 (Figur/Prop/Klassenwechsel) bzw. 0,6 (Boden–Boden), Klassenkante an Figuren, `hdr *= 1 - 0,92·edge`.
- **stylized:** Wrap 0,45, Terminator-Tönung `mix(S, 1, smoothstep(0, 0,6, d))`, normalisiertes Blinn-Phong; Rim `k·(1-N·V)^p·Farbe·(0,6+0,4·saturate(N.z+0,3))` nur an Figuren: Spieler p 3,0 k 0,55 #A8E6FF, Imps p 2,5 k 0,35 #FF9A6A, Brutes p 2,5 k 0,40 #FF9A6A.
- **realistic:** Cook-Torrance (GGX, höhenkorreliertes Smith, Schlick), Rauheit ≥ 0,25, Ambient `amb(N)·albedo·(1-m) + amb(R)·EnvBRDFApprox`.

| Material | Albedo | Schattenton (stylized) | Glanz g | ks | Rauheit | Metall |
|---|---|---|---:|---:|---:|---:|
| floor stone | #33363D | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| grout | #1A1C21 | #3A3F66 | 12 | 0.06 | 0.95 | 0 |
| pillar stone | #3E4047 | #3A3F66 | 12 | 0.06 | 0.75 | 0 |
| plinth | #2C2E34 | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| ruin wall | #383A3F | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| moss | #2E3A2A | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| rubble | #45464A | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| altar basalt | #2A2528 | #2A1830 | 20 | 0.1 | 0.6 | 0 |
| brazier bronze | #6B4A2A | #5A3A20 | 32 | 0.5 | 0.35 | 1 |
| candle wax | #D8CDB4 | #3A3F66 | 12 | 0.06 | 0.85 | 0 |
| cloak | #24505C | #1E2A4A | 6 | 0.03 | 0.9 | 0 |
| hood inside | #0A0C0E | #1E2A4A | 6 | 0.03 | 0.9 | 0 |
| bone mask | #CFC3A8 | #1E2A4A | 16 | 0.08 | 0.6 | 0 |
| staff wood | #4A3524 | #1E2A4A | 10 | 0.05 | 0.7 | 0 |
| imp ash skin | #7A7068 | #4A3550 | 20 | 0.12 | 0.55 | 0 |
| imp horns | #C9BBA0 | #4A3550 | 20 | 0.12 | 0.55 | 0 |
| brute rust flesh | #5E3530 | #3A1F3A | 18 | 0.1 | 0.6 | 0 |
| brute shoulder plates | #3B3A3E | #3A1F3A | 40 | 0.3 | 0.4 | 0 |

## Tuning-Protokoll und Korrekturen vor dem ersten gültigen Rendern

Keine Tuning-Runde nach dem Betrachten der Bilder. Vor dem ersten gültigen Rendern korrigiert:

- **Fehlerbehebung toon-Bänder:** alt 0.02 / 0.2 (Mindestkantenbreite 0.004), neu 0.004 / 0.04 (Mindestkantenbreite 0.0008). Die alte obere Schwelle war unter den Säulenfackeln unerreichbar: der Boden unter einer Fackel in 3,8 Höhe (Radius 9) erreicht höchstens x ≈ 0,061. Alle drei Werte sind mit demselben Faktor 0.2 skaliert, damit ihre Verhältnisse bleiben; die obere Schwelle liegt bei zwei Dritteln dieses Maximums, die Bandkanten unter einer Säulenfackel bei etwa 2,0 und 6,0 Einheiten Abstand. Weiter 3 Bänder, Beitragsfaktor 0,3 unverändert.
- **Kalibrierung:** Belichtungsskalar im Post-Stack ersetzt durch eine Lichtverstärkung auf die Lichtterme (siehe Lichtkalibrierung). Der Skalar hatte die gemeinsamen Emissives mitskaliert: Flammen waren in toon 1,65x heller, eigene Bolts liefen ins Weiße.
- **calm-Bullets:** 3 Spiralarme mit 12 statt 20 Reiskörnern, damit calm die entworfenen 180 Bullets hat (36 + 36 + 90 + 18). Die 18 verstreuten Orbs sind die calm-Stressplatzierungen (6 Fackelkegel, 6 Ritualkreis, 3 am Spieler, 3 über Säulen); busy nimmt seine aus dem Driftfeld.

## Auslegungen (für alle Looks gleich)

- Umgestürzte Säule bei (8.5, -4) statt (10, −6): an der Spezifikationsposition läuft sie durch den Sockel der intakten Säule bei 330°. Das Glutlicht wandert mit.
- Mauerbogen bei (-7.45, 9.46) statt (-9, 8): der tangential zum Arenakreis liegende Bogen (Radius 12,04, Länge 5) lief durch Säule und Sockel bei 150°. Er ist entlang seines eigenen Bogens verschoben (Radius und tangentiale Ausrichtung bleiben), um das Minimum für mindestens 1 Einheit Abstand zum Sockel: 2,12 Einheiten, Abstand jetzt 1,01.
- Gebrochene Säulen (90° und 210°) tragen ihre Fackel knapp unter der Bruchkante statt in 3,6 Höhe.
- Kalibrierziel: siehe Lichtkalibrierung; `--floor-space linear` rendert die lineare Lesart.
- busy: Randkerzen, Runensteine und schwebende Glut bekommen sichtbare Emissive-Punkte (World-Layer, in allen Looks gleich).
- Hintergrund: die Clear-Farbe ist durch den Tonemapper zurückgerechnet, damit #06070A ankommt.

## Befunde (berichtet, nicht getunt)

- **Fuß von PBR Neutral:** für einen Minimum-Kanal x < 0,08 zieht der Tonemapper `x − 6,25x²` von allen Kanälen ab. Das Kalibrierziel liegt darin: ein neutrales Grau braucht HDR 0.066, um als 0.0272 (linear) anzukommen, also 2.4x dunkler. Im Fuß wächst die Luminanz etwa quadratisch mit der Lichtverstärkung, und farbige dunkle Töne werden gesättigt, weil der kleinste Kanal relativ am stärksten sinkt. Das gilt für alle Looks gleich, drückt aber Schattenzeichnung und Terminator-Tönung (stylized) zusammen.
- **Kontrastgrenze von H0 Hexenmagenta:** Körper-Luminanz L = 0.266; 4,5:1 ist nur vor einem Hintergrund mit L < 0.020 möglich, also nur vor fast schwarzem Boden. Die magentafarbenen Cluster-Lichter tönen den Boden unter dichten Bullet-Wolken zusätzlich zur Bullet-Farbe hin. Die Lesbarkeit von H0 trägt der dunkle Rand #0A0510, nicht der Körper; das betrifft alle Looks, schadet kontrastarmen Looks aber mehr.

## Grenzen des Spikes (nicht dem Look anzulasten)

- Prozedurale Proxys ohne Texturen, Normal-Maps und Schatten: benachteiligt **realistic** am stärksten (GGX lebt von Materialdetail; ohne sie wirkt es wie Ton oder Plastik).
- Einfache konvexe Proxys schmeicheln dem **stylized**-Rimlight und geben **toon**-Outlines saubere Silhouetten; Blender-Modelle mit Konkaven und Stofffalten werden beides unruhiger machen.
- Kein Clustering: jedes Fragment läuft über alle Lichter mit gleichem Radius-Early-out; das staucht die Kostenabstände.
- Standbilder zeigen keine zeitliche Stabilität (Outline- und Specular-Flimmern); dafür die 1x-Ausschnitte.
- Engine-Grenzen gelten für alle Looks gleich: `Features::empty()` und WebGL2-Downlevel-Limits, Lichter in einem Uniform-Array (max. 256), keine Storage-Buffer. Boden 51200 Dreiecke.
