# Look-Dev-Spike: Messwerte

Erzeugt von `spikes/look-dev/render_all.sh`. Eine Szene, ein Seed, eine Kamera, eine Materialtabelle; die Looks unterscheiden sich nur im Fragment-Einstieg (`fs_toon`, `fs_stylized`, `fs_realistic`) und im Outline-Pass von toon.

Adapter: Microsoft Basic Render Driver (Cpu, Dx12) (Software-Adapter).

## Legende

- `toon`: Toon (Cel-Shading, 3 Lichtbänder, Outlines)
- `stylized`: Stilisiertes 3D (weiches Licht, Rimlight, ohne Outlines)
- `realistic`: Realistischer (GGX-Mikrofacetten, ohne Rim und Outlines)

`composite_side_by_side.png` (1920x720): Spalten von links nach rechts **toon | stylized | realistic**, Zeilen **calm oben, busy unten**; Zellen 640x360 (Frames linear halbiert), 4-px-Stege #000000 über den Zellkanten. **Lesbarkeit nicht am Composite beurteilen:** die Halbierung schrumpft Bullets auf 3–5 px; dafür die 1280x720-Frames und `composite_crops.png`.

- `composite_crops.png` (1920x360): busy in nativer Auflösung (2x SSAA), Ausschnitt x 320–960, y 300–660; Spalten toon | stylized | realistic.
- `<look>_<variant>.png`: finaler Frame. `<look>_<variant>_world.png`: nach PostFxResolve, vor Bullets und Marker (Grundlage der Kontrastmessung).
- `<look>_busy_1x_crop.png`: 640x360, **ohne** SSAA gerendert (Produktions-AA), Ausschnitt zwischen Spieler und nächstem sichtbaren Fackelkegel: toon ab (285, 182); stylized ab (285, 182); realistic ab (285, 182);
- `bullets_only_busy.png`: Bullet-Pass und Marker auf transparentem Schwarz.

## Lichtkalibrierung

Kein Belichtungsskalar im Post-Stack (Belichtung 1,0 für alle). Der Shader jedes Looks multipliziert das **direkte Licht** (Punktlichter und Mond; Diffus und Glanz) mit **globalem Lichtintensitätsfaktor 8 × Lichtverstärkung des Looks**, das **Hemisphären-Ambient nur mit dem Faktor** (für alle Looks gleich). Der Faktor ist so gesetzt, dass realistic eine Verstärkung von etwa 1,0 bekommt. Nicht skaliert werden Rimlight (stylized), Emissive-Meshes (Flammen, Augen, Orb), Ritualkreis-Emission, eigene Bolts und Bullets: ihre HDR-Multiplikatoren behalten die entworfene Bedeutung.

Die Verstärkung je Look ist so gewählt, dass der Median der Boden-Luminanz (Klasse 0, ohne Ritualkreis und Blob-Schatten) im calm-World-only-Bild 0.18 ±5 % trifft, als sRGB-kodierte relative Luminanz (Anzeigewert) (linear 0.0272); Abbruch bei 1 % Abweichung. busy nutzt dieselbe Verstärkung unverändert. Gleicher Median heißt nicht gleiche Lichter: die Verteilung der Boden-Luminanz (p10 bis p99) je Look und Variante steht unter „Boden-Luminanz“.

| Look | Lichtverstärkung | wirksame Lichtskala (Faktor × Verstärkung) | Boden-Median calm (linear) | Iterationen |
|---|---:|---:|---:|---:|
| toon | 1.373 | 10.982 | 0.0271 | 3 |
| stylized | 0.715 | 5.721 | 0.0272 | 3 |
| realistic | 1.000 | 8.000 | 0.0270 | 1 |

## Bullet-Kontrast

WCAG-Kontrast der Bullet-Körperfarbe gegen die mittlere Luminanz eines Rings 3–6 px außerhalb des projizierten Radius im World-only-Bild (kreisförmig um den großen Radius, auch bei Reis). Bullets ohne Ringpixel im Bild zählen nicht. Die Randfarbe #0A0510 steht zum Vergleich daneben. Obergrenzen: H0 Hexenmagenta (L = 0.266) erreicht 4,5:1 nur vor einem Hintergrund mit L < 0.020, H1 Giftlimette (L = 0.817) bis L < 0.143.

Zusätzlich die Doppelmetrik aus dem Bullet-Design: je Bullet der höhere Kontrast von Körper und dunklem Rand (1,5 px) gegen den Hintergrund.

| Look | Variante | Bullets | min | 5. Perz. | Median | Anteil ≥ 4,5:1 | Median H0 Magenta | Median H1 Limette | Median Rand | Anteil ≥ 4,5:1 Körper oder Rand |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| toon | calm | 180 | 1.04 | 1.44 | 3.94 | 43 % | 2.69 | 6.90 | 2.27 | 51 % |
| toon | busy | 1921 | 1.00 | 1.24 | 3.24 | 34 % | 2.12 | 5.76 | 2.87 | 52 % |
| stylized | calm | 180 | 1.06 | 1.37 | 3.80 | 39 % | 2.51 | 5.90 | 2.65 | 54 % |
| stylized | busy | 1921 | 1.00 | 1.09 | 2.58 | 21 % | 1.68 | 3.95 | 3.95 | 62 % |
| realistic | calm | 180 | 1.11 | 1.41 | 3.79 | 41 % | 2.68 | 6.18 | 2.50 | 52 % |
| realistic | busy | 1921 | 1.00 | 1.12 | 2.66 | 22 % | 1.73 | 4.14 | 3.77 | 58 % |

## Boden-Luminanz

Lineare relative Luminanz der Bodenpixel (Klasse 0, ohne Ritualkreis und Blob-Schatten) im World-only-Bild. Letzte Spalte: Anteil über L = 0.143, bis zu dem H1 Limette noch 4,5:1 erreicht.

| Look | Variante | p10 | p50 | p90 | p99 | Anteil über Limetten-Grenze |
|---|---|---:|---:|---:|---:|---:|
| toon | calm | 0.0103 | 0.0271 | 0.1119 | 0.3827 | 8 % |
| toon | busy | 0.0199 | 0.0471 | 0.1973 | 0.5056 | 15 % |
| stylized | calm | 0.0106 | 0.0272 | 0.1099 | 0.3570 | 6 % |
| stylized | busy | 0.0151 | 0.0641 | 0.2264 | 0.4981 | 21 % |
| realistic | calm | 0.0121 | 0.0270 | 0.0923 | 0.2712 | 5 % |
| realistic | busy | 0.0234 | 0.0648 | 0.1948 | 0.4307 | 19 % |

## Figur-gegen-Hintergrund-Kontrast

Aus der gemeinsamen Klassenmaske: Ring 2–6 px außerhalb der Silhouette (ohne Pixel anderer Figuren) gegen (a) die mittlere Luminanz der beleuchteten Silhouette ohne Augen/Orb, (b) die mittlere Luminanz des inneren Konturbands 0–3 px und (c) **Kante:** Median des Kontrasts je Pixel im Randband 0–1,5 px gegen den Ring. (b) mittelt eine dunkle Outline mit dem hellen Körper weg, (c) nicht. 9 Figuren.

| Look | Variante | Median gesamt | min gesamt | Median Kontur | min Kontur | Median Kante | min Kante |
|---|---|---:|---:|---:|---:|---:|---:|
| toon | calm | 1.99 | 1.02 | 1.90 | 1.05 | 1.93 | 1.48 |
| toon | busy | 1.80 | 1.21 | 1.70 | 1.00 | 2.18 | 1.46 |
| stylized | calm | 2.43 | 1.06 | 2.16 | 1.06 | 2.22 | 1.23 |
| stylized | busy | 2.09 | 1.12 | 1.92 | 1.05 | 1.94 | 1.17 |
| realistic | calm | 1.53 | 1.18 | 1.49 | 1.07 | 1.58 | 1.44 |
| realistic | busy | 1.67 | 1.24 | 1.43 | 1.08 | 1.57 | 1.26 |

Je Figur (gesamt / Kontur / Kante), Reihenfolge: 1 Spieler, 2–7 Imps, 8–9 Brutes:

| Look | Variante | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
|---|---|---|---|---|---|---|---|---|---|---|
| toon | calm | 1.71 / 2.19 / 4.85 | 1.31 / 1.08 / 1.79 | 1.47 / 1.18 / 1.93 | 2.40 / 1.90 / 2.03 | 3.25 / 2.28 / 1.63 | 3.61 / 2.52 / 1.96 | 4.13 / 3.06 / 1.83 | 1.99 / 1.05 / 1.48 | 1.02 / 1.61 / 2.35 |
| toon | busy | 1.52 / 1.97 / 4.68 | 1.22 / 1.00 / 1.46 | 1.29 / 1.16 / 2.18 | 2.78 / 2.05 / 2.19 | 3.20 / 2.53 / 2.24 | 3.64 / 2.64 / 2.01 | 2.26 / 1.70 / 1.89 | 1.80 / 1.02 / 1.54 | 1.21 / 1.68 / 2.94 |
| stylized | calm | 1.06 / 1.06 / 1.31 | 1.31 / 1.29 / 1.23 | 1.57 / 1.52 / 1.53 | 3.07 / 2.99 / 2.66 | 3.27 / 3.02 / 2.98 | 4.96 / 4.25 / 3.63 | 5.09 / 4.34 / 4.42 | 2.43 / 2.16 / 2.22 | 1.12 / 1.22 / 1.33 |
| stylized | busy | 1.12 / 1.10 / 1.20 | 1.27 / 1.25 / 1.25 | 1.36 / 1.39 / 1.32 | 3.60 / 3.30 / 2.85 | 3.21 / 3.05 / 2.90 | 4.44 / 3.96 / 3.47 | 2.30 / 2.03 / 2.04 | 2.09 / 1.92 / 1.94 | 1.26 / 1.05 / 1.17 |
| realistic | calm | 1.18 / 1.17 / 1.52 | 1.25 / 1.24 / 1.46 | 1.53 / 1.49 / 1.44 | 2.58 / 2.28 / 1.58 | 2.91 / 2.47 / 2.11 | 4.11 / 3.31 / 2.32 | 4.39 / 3.54 / 2.74 | 1.39 / 1.07 / 1.52 | 1.37 / 1.41 / 1.64 |
| realistic | busy | 1.25 / 1.19 / 1.56 | 1.24 / 1.24 / 1.26 | 1.35 / 1.43 / 1.37 | 3.07 / 2.65 / 2.13 | 2.97 / 2.87 / 2.55 | 3.58 / 3.09 / 2.22 | 2.14 / 1.83 / 1.53 | 1.28 / 1.08 / 1.66 | 1.67 / 1.20 / 1.57 |

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

**Software-Adapter (WARP), kein GPU-Budget.** CPU-Wandzeit um Queue-Submit plus `device.poll(Wait)` je Schritt, relativ zu toon derselben Variante. Jede Runde ist ein eigener kurzer Prozess: jeder Look wird zuerst einmal ungemessen gerendert, dann werden alle drei in je Runde rotierter Reihenfolge gemessen. 3 Aufwärmrunden verworfen, Median aus 15 Runden. Die CPU-Rasterisierung verzerrt auch relative Kosten (SIMD-freundliche Mathematik gegen Verzweigungen und `pow`), und die Schleife über alle Lichter ohne Clustering staucht die Abstände. Das Normale+Klasse-MRT (nur die toon-Outline braucht es) und das teure gemeinsame `surface()` (Bodenrelief-Hashes, Ritualkreis-SDF, Blob-Schleife) laufen in allen Looks und stauchen die Verhältnisse weiter. Seit der Polish-Runde quantisiert toon einmal nach der Lichtschleife (keine analytischen Ableitungen je Licht mehr), hat aber einen harten Glanzpunkt je Licht.

| Variante | Look | World-Pass | Outline-Pass (Anteil am toon-Frame) | Post + Bullets | Gesamt |
|---|---|---:|---:|---:|---:|
| calm | toon | 1.00 | 0.07 | 1.00 | 1.00 |
| calm | stylized | 0.91 | 0.00 | 1.00 | 0.87 |
| calm | realistic | 0.95 | 0.00 | 1.00 | 0.89 |
| busy | toon | 1.00 | 0.02 | 1.00 | 1.00 |
| busy | stylized | 0.86 | 0.00 | 1.00 | 0.85 |
| busy | realistic | 0.98 | 0.00 | 1.01 | 0.95 |

## Eingefrorene Look-Parameter

Gemeinsam: Albedo je Material (Tabelle unten), Licht-Falloff `saturate(1-(d/r)^4)^2/(1+d^2)`, Mond #9AB0D8 I 0,35 entlang (-0,35, 0,5, -0,8), Hemisphären-Ambient Himmel #1C2438 / Boden #110D0B I 0,35, Bloom Schwelle 1,0 / Knie 0,5 / 4 Stufen / 0,08, Khronos PBR Neutral, 2x SSAA.

- **toon:** Lambert-Bestrahlung (Mond + Punktlichter) wie in den anderen Looks aufsummiert und **einmal nach der Lichtschleife** in absoluter Luminanz Y (Lichtverstärkung 1) quantisiert: Schwellen 0.566 / 1.357 (25 % / 60 % der Bodenluminanz 2.262 unter einer intakten Säulenfackel, Mond eingeschlossen), Stufen 0 / 1.034 / 1.692 (flächengewichtetes Mittel der weichen Antwort im mittleren und oberen Band dieses Fackelkegels), Kantenbreite `max(fwidth(Y), 0.01·Schwelle 2)`, Farbe `E·R(Y)/Y`. Schattenband: nur Ambient mit Schattenton (wie stylized). Harter Glanzpunkt je Licht: Stufe von (N·H)^g bei 0.5 (±0,05), Höhe `ks·(g+8)/8·0.72`, Farbe wie stylized. Outline: symmetrisches Kreuz 2 SS-px (1x: 1 px), Tiefe `smoothstep(0,010, 0,020)` relativ, Normale 0,35 (Figur/Prop/Klassenwechsel) bzw. 0,6 (Boden–Boden) je ±0,05, Klassenkante an Figuren, Linie `mix(hdr, #0A090C, edge)`.
- **stylized:** Wrap 0,45, Terminator-Tönung `mix(S, 1, smoothstep(0, 0,6, d))`, normalisiertes Blinn-Phong; Rim `k·(1-N·V)^p·Farbe·(0,6+0,4·saturate(N.z+0,3))` nur an Figuren, Farbe #D8ECFF für alle: Spieler p 3,0 k 0,55, Imps p 2,5 k 0,35, Brutes p 2,5 k 0,40. Der Rim ist ein reiner Fresnel-Term der Blickrichtung: er hängt von keinem Licht ab und leuchtet auch in dunklen Zonen.
- **realistic:** Cook-Torrance (GGX, höhenkorreliertes Smith, Schlick), F0 = `mix(0,04, Metall-F0, m)`, Rauheit ≥ 0,25, geometrisches Specular-Anti-Aliasing (`α² += min(2·(dN_x²+dN_y²)/(2π), 0,18)`), Ambient `amb(N)·albedo·(1-m)·(1-EnvBRDF) + amb(R)·EnvBRDF` (EnvBRDFApprox nach Karis).

| Material | Albedo | Schattenton (stylized, toon) | Glanz g | ks | Rauheit | Metall | Metall-F0 |
|---|---|---|---:|---:|---:|---:|---|
| floor stone | #33363D | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| grout | #1A1C21 | #3A3F66 | 12 | 0.06 | 0.95 | 0 | – |
| pillar stone | #3E4047 | #3A3F66 | 12 | 0.06 | 0.75 | 0 | – |
| plinth | #2C2E34 | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| ruin wall | #383A3F | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| moss | #2E3A2A | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| rubble | #45464A | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| altar basalt | #2A2528 | #2A1830 | 20 | 0.1 | 0.6 | 0 | – |
| brazier bronze | #6B4A2A | #5A3A20 | 32 | 0.5 | 0.35 | 1 | #F2C28A |
| candle wax | #D8CDB4 | #3A3F66 | 12 | 0.06 | 0.85 | 0 | – |
| cloak | #24505C | #1E2A4A | 6 | 0.03 | 0.9 | 0 | – |
| hood inside | #0A0C0E | #1E2A4A | 6 | 0.03 | 0.9 | 0 | – |
| bone mask | #CFC3A8 | #1E2A4A | 16 | 0.08 | 0.6 | 0 | – |
| staff wood | #4A3524 | #1E2A4A | 10 | 0.05 | 0.7 | 0 | – |
| imp ash skin | #7A7068 | #4A3550 | 20 | 0.12 | 0.55 | 0 | – |
| imp horns | #C9BBA0 | #4A3550 | 20 | 0.12 | 0.55 | 0 | – |
| brute rust flesh | #5E3530 | #3A1F3A | 18 | 0.1 | 0.6 | 0 | – |
| brute shoulder plates | #3B3A3E | #3A1F3A | 40 | 0.3 | 0.4 | 0 | – |

## Korrekturprotokoll

Keine Tuning-Runde nach dem Betrachten der Bilder: jede Einstellung unten folgt aus einem Review-Befund oder einer Herleitung, nicht aus einem Bildvergleich.

### Polish-Runde nach zwei Reviews (Art und Technik), vor diesem Lauf

- **Cluster-Lichter nicht mehr in Bullet-Farben (alle Looks):** die 6 calm- und 64 busy-Cluster-Lichter hatten die Körperfarbe ihrer Bullets (Magenta, Limette) und tönten den Boden im Palettenraum der Bullets, was PRD-0003 für die Umgebung ausschließt. Jetzt Farbton #B07850 (entsättigte Glut) bei gleicher Luminanz wie zuvor.
- **toon-Lichtmodell neu (Behinderung behoben):** vorher Bänder je Licht auf `saturate(N·L)·atten` bei 0.004 / 0.04 ohne Lichtintensität, Stufen 0,45 / 1,0 × 0,3 je Licht aufsummiert. Das machte Kegelgrößen unabhängig von der Lichtstärke, lieferte am Kegelrand bis 34-mal die Energie der weichen Looks und stapelte in busy Ringe. Jetzt wie oben unter „Eingefrorene Look-Parameter“: einmal quantisierte Lambert-Bestrahlung. Die analytische `fwidth`-Nachbildung je Licht entfällt.
- **Lichtverstärkung aufgeteilt (alle Looks):** vorher skalierte die Verstärkung auch das Ambient, toon bekam dadurch nur 41 % des Ambient von realistic und dunkle Figuren. Jetzt skaliert sie nur das direkte Licht; das Ambient ist in allen Looks gleich.
- **toon-Extras aus derselben Materialtabelle:** Schattenton im Schattenband und harter Glanzpunkt (stylized hatte Tönung und Glanz, toon nicht).
- **toon-Outline:** vorher binäre `step()`-Kante, asymmetrische Taps −1/+2 (Linie nach rechts unten versetzt, fraß in kleine Imps), 3 SS-px gegen 2 px bei 1x, Multiplikation `hdr·(1−0,92·edge)`. Jetzt kontinuierliche Kantenstärke, symmetrisches Kreuz, gleiche Linienbreite in Endpixeln bei 1x und 2x, absolute Linienfarbe.
- **stylized-Rim:** vorher farbgleich mit den Figuren (Spieler #A8E6FF auf cyanem Umhang, Imps und Brutes #FF9A6A auf orangen Körpern unter warmem Fackellicht) und kaum sichtbar. Jetzt neutral kühles #D8ECFF für alle Figuren; Stärke k und Exponent p unverändert (keine Stärken-Tuning-Runde).
- **realistic:** Bronze nutzte die dunkle Diffus-Albedo #6B4A2A als F0 und wurde fast schwarz; jetzt eigene Metall-F0-Spalte (Bronze #F2C28A, auch Glanzfarbe in stylized und toon). Diffuses Ambient mit `(1−EnvBRDF)` gewichtet (vorher leicht doppelt gezählt), geometrisches Specular-Anti-Aliasing gegen Funkeln der Kachelnormalen.
- **Blob-Schatten (alle Looks):** 60 % statt 45 % Abdunklung in der Mitte, damit Figuren weniger schweben.
- **Messwerte ergänzt:** Boden-Luminanz p10–p99, Bullet-Doppelmetrik Körper oder Rand, outline-taugliches Kantenmaß für Figuren.

Aus den Reviews **nicht** umgesetzt (offen): zweiter Rendersatz mit Kalibrierung auf busy-p90, Telegraph-Fixture (Telegraph-Ebene bleibt leer), Kreuzvarianten toon+Rim und stylized+Outline, Fugen-Varianten des Bodens, zweites Stimmungsziel 0,12, andere Umhangfarbe, lichtabhängiger Rim, Kostenschalter (MRT nur für toon, flaches `surface()`).

### Vor dem ersten gültigen Rendern

- **toon-Bänder (damals):** die spezifizierten Schwellen 0,02 / 0,20 waren unter den Säulenfackeln unerreichbar und wurden auf 0,004 / 0,04 skaliert. Durch die Polish-Runde überholt.
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
- **Kontrastgrenze von H0 Hexenmagenta:** Körper-Luminanz L = 0.266; 4,5:1 ist nur vor einem Hintergrund mit L < 0.020 möglich, also nur vor fast schwarzem Boden. Die Lesbarkeit von H0 trägt der dunkle Rand #0A0510, nicht der Körper (siehe Doppelmetrik); das ist eine Frage der Palette, nicht des Looks.

## Grenzen des Spikes (nicht dem Look anzulasten)

- Prozedurale Proxys ohne Texturen, Normal-Maps und Schatten: benachteiligt **realistic** am stärksten (GGX lebt von Materialdetail; ohne sie wirkt es wie Ton oder Plastik).
- Einfache konvexe Proxys schmeicheln dem **stylized**-Rimlight und geben **toon**-Outlines saubere Silhouetten; Blender-Modelle mit Konkaven und Stofffalten werden beides unruhiger machen.
- Kein Clustering: jedes Fragment läuft über alle Lichter mit gleichem Radius-Early-out; das staucht die Kostenabstände.
- Standbilder zeigen keine zeitliche Stabilität (Outline- und Specular-Flimmern); dafür die 1x-Ausschnitte.
- Keine Telegraph-Ebene (Schritt 5 im Renderer ist reserviert und leer): ob Telegraphen sich von der Umgebung abheben, sagen die Bilder nicht. Der violette Ritualkreis sieht einem Telegraphen ähnlich.
- Die Looks bündeln zwei Variablen: Shading-Modell (Bänder gegen weich) und Trennmittel (Outline gegen Rim). Kreuzvarianten gibt es nicht.
- Keine Schattenkarten, nur Blob-Schatten; die kachelgleiche prozedurale Fuge erzeugt in allen Looks ein hochfrequentes Muster hinter Bullets.
- Engine-Grenzen gelten für alle Looks gleich: `Features::empty()` und WebGL2-Downlevel-Limits, Lichter in einem Uniform-Array (max. 256), keine Storage-Buffer. Boden 51200 Dreiecke.
