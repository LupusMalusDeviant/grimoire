# Look-Vergleich in Blender Eevee: Messwerte

Legende der Komposite: Spalten von links nach rechts **toon | stylized | realistic**; Zeilen **calm oben, busy unten**. `composite_crops.png`: busy, native Ausschnitte x 320-960, y 300-660, gleiche Spaltenreihenfolge.

## Kalibrierung (Licht-Gain je Look)

Globaler Lichtfaktor (Design-Einheiten zu Blender): 8,167. Ziel: Median der Boden-Leuchtdichte als Anzeigewert 0,18 (±5 %).

| Look | Licht-Gain | entspricht Blendenstufen | erreichter Anzeige-Median (calm, mit Bloom) |
|---|---|---|---|
| toon | 1,456 | 0,54 | 0,182 |
| stylized | 0,867 | -0,21 | 0,182 |
| realistic | 1,000 | 0,00 | 0,181 |

## Bullet-Kontrast (WCAG, Körperfarbe gegen Ring 3-6 px im Welt-Bild)

| Look | Variante | n | Min | 5. Perzentil | Median | Anteil ≥ 4,5:1 |
|---|---|---|---|---|---|---|
| toon | calm | 180 | 1,32 | 1,89 | 4,52 | 50,0 % |
| toon | busy | 1983 | 1,08 | 1,99 | 4,17 | 48,8 % |
| toon | busy_dim | 1983 | 1,22 | 2,07 | 4,52 | 50,2 % |
| stylized | calm | 180 | 1,01 | 1,16 | 3,33 | 30,0 % |
| stylized | busy | 1983 | 1,00 | 1,06 | 1,97 | 11,0 % |
| stylized | busy_dim | 1983 | 1,00 | 1,11 | 2,96 | 26,2 % |
| realistic | calm | 180 | 1,02 | 1,20 | 3,51 | 36,7 % |
| realistic | busy | 1983 | 1,00 | 1,07 | 2,19 | 13,1 % |
| realistic | busy_dim | 1983 | 1,01 | 1,17 | 3,24 | 33,0 % |

bullets_rejected_palette_space: calm 0, busy 0 (muss 0 sein). Die Bullet-Ebene wird einmal je Variante gezeichnet und identisch über alle Looks gelegt (SHA-256 busy: `730547d993b04189`).

## Figur-gegen-Hintergrund-Kontrast (Klassenmaske, Ring 2-6 px außerhalb der Silhouette)

| Look | Variante | Median | Min | schwächste Figur |
|---|---|---|---|---|
| toon | calm | 3,28 | 1,82 | Brute 2 |
| toon | busy | 3,46 | 1,44 | Brute 2 |
| toon | busy_dim | 3,50 | 1,56 | Brute 2 |
| stylized | calm | 2,62 | 1,23 | Imp 2 |
| stylized | busy | 1,72 | 1,23 | Imp 2 |
| stylized | busy_dim | 2,82 | 1,31 | Imp 2 |
| realistic | calm | 2,02 | 1,56 | Imp 2 |
| realistic | busy | 1,67 | 1,19 | Brute 2 |
| realistic | busy_dim | 2,15 | 1,51 | Imp 1 |

## Variante busy_dim (Cluster-Lichter auf 25 %)

Identisch mit busy (Geometrie, Bullets, Bullet-Ebene, Licht-Gains ohne Neukalibrierung); nur die 64 Bullet-Cluster-Lichter der unruhigen und die 6 der ruhigen Variante leuchten mit 25 % ihrer Intensität. Test einer Szenen-Eigenschaft, für alle Looks gleich. Komposite: `composite_busy_dim.png` (oben busy, unten busy_dim) und `composite_crops_busy_dim.png`.

| Look | Bullet-Median busy → dim | P5 busy → dim | Anteil ≥ 4,5:1 busy → dim | Figur-Median busy → dim | Figur-Min busy → dim |
|---|---|---|---|---|---|
| toon | 4,17 → 4,52 | 1,99 → 2,07 | 48,8 % → 50,2 % | 3,46 → 3,50 | 1,44 → 1,56 |
| stylized | 1,97 → 2,96 | 1,06 → 1,11 | 11,0 % → 26,2 % | 1,72 → 2,82 | 1,23 → 1,31 |
| realistic | 2,19 → 3,24 | 1,07 → 1,17 | 13,1 % → 33,0 % | 1,67 → 2,15 | 1,19 → 1,51 |

## Renderzeit je Finalbild

Messung: Blender Eevee auf der GPU des Entwicklungsrechners, kein Engine-Budget. Median aus 3 Läufen nach 1 Aufwärmlauf, gemessen um `bpy.ops.render.render` (inklusive Compositor, ohne Dateischreiben).

Die Einzelwerte streuen stark (asynchrone Shader-Kompilierung von Eevee reicht über den einen Aufwärmlauf hinaus). Deshalb steht daneben eine Kontrollmessung mit 3 Aufwärm- und 5 Messläufen im selben Ablauf; nur sie taugt für einen Vergleich zwischen den Looks, und auch sie ist kein Engine-Budget.

| Look | Variante | Median s (1+3) | Einzelwerte s | Kontrolle Median s (3+5) | Kontrolle Einzelwerte s |
|---|---|---|---|---|---|
| toon | calm | 1,14 | 1,86, 1,14, 1,04 | 0,31 | 0,30, 0,30, 0,32, 0,32, 0,31 |
| toon | busy | 0,40 | 0,40, 0,40, 0,40 | 0,42 | 0,41, 0,42, 0,42, 0,42, 0,42 |
| toon | busy_dim | 0,42 | 0,42, 0,42, 0,43 | - | - |
| stylized | calm | 0,30 | 0,30, 0,30, 0,33 | 0,31 | 0,30, 0,31, 0,31, 0,31, 0,32 |
| stylized | busy | 0,41 | 0,41, 0,41, 0,40 | 0,43 | 0,43, 0,42, 0,44, 0,43, 0,42 |
| stylized | busy_dim | 0,44 | 0,44, 0,44, 0,44 | - | - |
| realistic | calm | 0,27 | 0,28, 0,27, 0,27 | 0,27 | 0,27, 0,27, 0,27, 0,27, 0,28 |
| realistic | busy | 0,34 | 0,34, 0,33, 0,34 | 0,35 | 0,35, 0,35, 0,35, 0,35, 0,34 |
| realistic | busy_dim | 0,34 | 0,35, 0,34, 0,34 | - | - |

## Tuning-Log

| Look | Parameter | alt | neu | Begründung |
|---|---|---|---|---|
| toon | outline_thickness | 0.04 | 0.05 | Linien maßen im Finalbild etwa 1,3 px (0,04 Einheiten bei rund 33 px pro Einheit) und lagen damit unter dem Ziel von 1,5-2 px; 0,05 ergibt etwa 1,65 px. Dünne Teile (Stab, Kerzen, Dreibeine) skalieren mit 0,5. |
| stylized | (keine Änderung) | - | - | Kein Fehlerbild; Rim, Wrap und Glanz lesen sich wie spezifiziert. Keine Anpassung, um nicht in Richtung eines Favoriten zu tunen. |
| realistic | (keine Änderung) | - | - | Kein Fehlerbild; die Plastik-Anmutung liegt an den untexturierten Proxies, nicht an Parametern. |

## Abweichungen vom Design

1. Ruhige Variante: 3 Spiralarme mit je 12 statt 20 Reis-Geschossen (die Design-Zählung ergibt 204 statt 180). Die 18 Streukugeln sind die Stress-Platzierungen (6 Fackelpool, 6 Ritualkreis, 3 Spielerrand, 3 Säulen). Unruhige Variante: die 60 Stress-Geschosse stammen aus dem 416er-Driftfeld.
2. Toon-Bänder und stilisierter Wrap wirken auf die gesamte direkte Beleuchtung (Eevee liefert über Shader to RGB nur die Summe aller Lichter), nicht pro Licht. Das Mondlicht wird mitquantisiert statt mit eigenen Schwellen 0,05/0,5.
3. Kalibrierung als Licht-Gain in der Look-Schattierung (wirkt auf Diffus, Ambient, Glanz; nicht auf Rim, Emissive, Dekal-Emission, freundliche Geschosse). Compositor-Belichtung neutral (0). Ziel: Median der Boden-Leuchtdichte als Anzeigewert 0,18 (sRGB nach View-Transform, linear etwa 0,027). Der globale Lichtfaktor ist so gewählt, dass der realistische Look Gain 1,0 hat.
4. Punktlicht-Abfall: Eevee rechnet 1/d² mit Fenster (1-(d/r)^4)² statt 1/(1+d²). Um das Design am Boden nachzubilden, sitzt jedes Punktlicht in der Höhe sqrt(h²+1) über dem Boden darunter (h = Design-Höhe) und cutoff_distance = sqrt(r²+1); der Boden erhält damit genau den Design-Abstandsterm. Ohne diese Anhebung brannten Randkerzen, Runensteine und Cluster-Lichter singuläre Hotspots (Bugfix im Entwurf, kein Tuning). Senkrechte Flächen direkt neben einer Lichtquelle bleiben heller als im Design. Ein globaler Watt-Faktor.
5. Echte Schattenwürfe (Eevee Shadow Maps) für Mond, 8 Fackeln/Kohlebecken und Stab-Orb in allen Looks; alle anderen Lichter ohne Schatten.
6. Geometrie: Plinthe (1,0) und Kapitell (0,9) als Halbmaße; Wandleuchter der gebrochenen Säulen bei z = min(3,6; Säulenhöhe - 0,5); Mauerbogen tangential, minimal nach (-7,9 | 10,0) verschoben (2,28 Einheiten), damit er Säule und Plinthe bei 150° um mindestens 1 Einheit freigibt; umgestürzte Säule samt Glutlicht bei (8,5 | -4,0) wie im Engine-Spike; Glutlicht auf z 0,25 statt Bodenkontakt (sonst singulärer Hotspot).
7. Toon-Outlines als invertierte Hülle (Solidify nach außen, gespiegelte Normalen, Backface-Culling, #0B0A0D, Dicke 0,04 Welteinheiten, dünne Teile 0,02) statt Screen-Space-Pass; nicht auf Boden, Dekal und Emissive.
8. Bloom: Blender-Glare im Bloom-Modus (Schwelle 1,0, Glättung 0,5, Stärke 0,08, Größe 0,1) statt 4-stufiger Dual-Filter-Kette; in den 1x-Crops wird Bloom nur auf dem Crop-Ausschnitt gerechnet. Kein 2x-SSAA, sondern 32 Eevee-Samples mit 1,5-px-Filter.
9. Stilisiert: Glanz über GGX Glossy BSDF (Rauheit aus dem Glanzexponenten, Farbe ks) statt normalisiertem Blinn-Phong; nimmt auch die schwache Welt-Umgebung als Reflexion auf.
10. Freundliche Geschosse als Kapsel-Meshes (Blended, Alpha 0,75) statt Billboards; die 64 Randkerzen der unruhigen Variante sind reine Lichter ohne Mesh.
11. Komposit-Größen 1928x724 und 1928x360 wegen 4-px-Rinnen zwischen den Zellen.

## Look-Parameter (vollständig)

```json
{
 "realistic": {
  "rough_min": 0.25
 },
 "stylized": {
  "rim": {
   "brute": {
    "color": "#FF9A6A",
    "k": 0.4,
    "p": 2.5
   },
   "imp": {
    "color": "#FF9A6A",
    "k": 0.35,
    "p": 2.5
   },
   "player": {
    "color": "#A8E6FF",
    "k": 0.55,
    "p": 3.0
   }
  },
  "spec_gain": 1.0,
  "tint_smooth": [
   0.0,
   0.6
  ],
  "wrap": 0.45,
  "wrap_ref": 0.15
 },
 "toon": {
  "band_full": 0.27,
  "band_levels": [
   0.0,
   0.45,
   1.0
  ],
  "edge_rel": 0.04,
  "hue_clamp": 8.0,
  "outline_color": "#0B0A0D",
  "outline_thickness": 0.05,
  "outline_thin_scale": 0.5,
  "t1": 0.03,
  "t2": 0.22
 }
}
```

### Materialtabelle (Albedo identisch in allen Looks)

```json
{
 "altar": {
  "albedo": "#2A2528",
  "g": 20,
  "ks": 0.1,
  "m": 0.0,
  "r": 0.6,
  "tint": "#2A1830"
 },
 "bronze": {
  "albedo": "#6B4A2A",
  "g": 32,
  "ks": 0.5,
  "m": 1.0,
  "r": 0.35,
  "spec_albedo": true,
  "tint": "#5A3A20"
 },
 "brute_flesh": {
  "albedo": "#5E3530",
  "g": 18,
  "ks": 0.1,
  "m": 0.0,
  "r": 0.6,
  "rim": "brute",
  "tint": "#3A1F3A"
 },
 "cloak": {
  "albedo": "#24505C",
  "g": 6,
  "ks": 0.03,
  "m": 0.0,
  "r": 0.9,
  "rim": "player",
  "tint": "#1E2A4A"
 },
 "floor_stone": {
  "albedo": "#33363D",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 },
 "grout": {
  "albedo": "#1A1C21",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.95,
  "tint": "#3A3F66"
 },
 "hood_inside": {
  "albedo": "#0A0C0E",
  "g": 6,
  "ks": 0.03,
  "m": 0.0,
  "r": 0.9,
  "rim": "player",
  "tint": "#1E2A4A"
 },
 "horn": {
  "albedo": "#C9BBA0",
  "g": 16,
  "ks": 0.08,
  "m": 0.0,
  "r": 0.6,
  "rim": "imp",
  "tint": "#4A3550"
 },
 "imp_skin": {
  "albedo": "#7A7068",
  "g": 20,
  "ks": 0.12,
  "m": 0.0,
  "r": 0.55,
  "rim": "imp",
  "tint": "#4A3550"
 },
 "mask": {
  "albedo": "#CFC3A8",
  "g": 16,
  "ks": 0.08,
  "m": 0.0,
  "r": 0.6,
  "rim": "player",
  "tint": "#1E2A4A"
 },
 "moss": {
  "albedo": "#2E3A2A",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 },
 "pillar": {
  "albedo": "#3E4047",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.75,
  "tint": "#3A3F66"
 },
 "plates": {
  "albedo": "#3B3A3E",
  "g": 40,
  "ks": 0.3,
  "m": 0.0,
  "r": 0.4,
  "rim": "brute",
  "tint": "#3A1F3A"
 },
 "plinth": {
  "albedo": "#2C2E34",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 },
 "rubble": {
  "albedo": "#45464A",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 },
 "ruin_wall": {
  "albedo": "#383A3F",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 },
 "staff_wood": {
  "albedo": "#4A3524",
  "g": 10,
  "ks": 0.05,
  "m": 0.0,
  "r": 0.7,
  "rim": "player",
  "tint": "#1E2A4A"
 },
 "wax": {
  "albedo": "#D8CDB4",
  "g": 12,
  "ks": 0.06,
  "m": 0.0,
  "r": 0.85,
  "tint": "#3A3F66"
 }
}
```

### Gemeinsamer Post-Stack und Einheiten

```json
{
 "calibration": {
  "achieved_display_median": {
   "realistic": 0.1806083552044979,
   "stylized": 0.1817426572399656,
   "toon": 0.1817426572399656
  },
  "gains": {
   "realistic": 1.0,
   "stylized": 0.8673383533797006,
   "toon": 1.4562976645214871
  },
  "history": [
   {
    "display_median_at_gain": 0.17999999999999863,
    "display_median_at_gain1": 0.13034430004061626,
    "floor_pixels": 679287,
    "gain": 1.4993079754794418,
    "light_scale": 5.5,
    "look": "realistic",
    "retargeted_light_scale": 8.24619386513693,
    "time_s": 2.524547099994379
   },
   {
    "display_median_at_gain": 0.1799999999999981,
    "display_median_at_gain1": 0.18011519040747137,
    "floor_pixels": 679287,
    "gain": 0.9992179197291289,
    "light_scale": 8.24619386513693,
    "look": "realistic",
    "stored_gain": 1.0,
    "time_s": 0.8791564999992261
   },
   {
    "display_median_at_gain": 0.1799999999999982,
    "display_median_at_gain1": 0.1376692414797373,
    "floor_pixels": 679287,
    "gain": 1.438205256885582,
    "light_scale": 8.24619386513693,
    "look": "toon",
    "time_s": 2.099490100008552
   },
   {
    "display_median_at_gain": 0.18000000000000246,
    "display_median_at_gain1": 0.19836814338891595,
    "floor_pixels": 679287,
    "gain": 0.8839879743137535,
    "light_scale": 8.24619386513693,
    "look": "stylized",
    "time_s": 3.4790088000008836
   },
   {
    "display_median_at_gain": 0.18000000000000071,
    "display_median_at_gain1": 0.18136524049451552,
    "floor_pixels": 679287,
    "gain": 0.9903709241955336,
    "light_scale": 8.24619386513693,
    "look": "realistic",
    "retargeted_light_scale": 8.166790639311202,
    "time_s": 0.8466402000049129
   },
   {
    "display_median_at_gain": 0.18000000000000274,
    "display_median_at_gain1": 0.1800388788112343,
    "floor_pixels": 679287,
    "gain": 0.9997161609276012,
    "light_scale": 8.166790639311202,
    "look": "realistic",
    "stored_gain": 1.0,
    "time_s": 0.8195636999880662
   },
   {
    "display_median_at_gain": 0.17999999999999766,
    "display_median_at_gain1": 0.13646908675810537,
    "floor_pixels": 679287,
    "gain": 1.4553218432352215,
    "light_scale": 8.166790639311202,
    "look": "toon",
    "time_s": 0.8872042000002693
   },
   {
    "display_median_at_gain": 0.1800000000000031,
    "display_median_at_gain1": 0.20213137085096963,
    "floor_pixels": 679287,
    "gain": 0.8673383533797006,
    "light_scale": 8.166790639311202,
    "look": "stylized",
    "time_s": 0.9191852000076324
   },
   {
    "display_median_at_gain": 0.18000000000000313,
    "display_median_at_gain1": 0.1363264125001391,
    "floor_pixels": 679287,
    "gain": 1.4562976645214871,
    "light_scale": 8.166790639311202,
    "look": "toon",
    "time_s": 3.897024899997632
   }
  ],
  "light_scale": 8.166790639311202,
  "stops": {
   "realistic": 0.0,
   "stylized": -0.2053331885020659,
   "toon": 0.5423052698482368
  },
  "units": {
   "k_point": 0.025330295910584444,
   "k_sun": 0.3183098861837907,
   "k_world": 1.0
  }
 },
 "post": {
  "bloom_quality": "High",
  "bloom_size": 0.1,
  "bloom_smoothness": 0.5,
  "bloom_strength": 0.08,
  "bloom_threshold": 1.0,
  "calib_erode_px": 2,
  "calib_exclude_radius": 6.2,
  "calib_tolerance": 0.05,
  "compositor_exposure": 0.0,
  "filter_final": 1.5,
  "look": "None",
  "samples_crop": 1,
  "samples_final": 32,
  "target_display_median": 0.18,
  "view_transform": "Khronos PBR Neutral"
 }
}
```

## Beobachtungen

### Toon (3 Bänder, invertierte Hülle)

- **Liest sich gut:** Die Bänder deckeln die Helligkeit der Lichtpools. Unter dichten Bullet-Wolken brennt der Boden deshalb nicht aus, und der Bullet-Kontrast bleibt in der unruhigen Variante mit Abstand am höchsten. Die Outlines schließen jede Figur, dadurch ist der Figur-gegen-Hintergrund-Kontrast der beste der drei Looks. Mondschatten erscheinen als klares dunkles Band.
- **Schwach:** Mit 232 Lichtern zerfällt der Boden in viele flache, bunte Flächen (Magenta, Limette, Orange) und wirkt unruhig. Die Brutes lesen sich als flache rote Scheiben ohne Volumen, weil ihr eigenes Augenlicht sie ins obere Band hebt. An dünnen Teilen (Stab, Dreibeine) sind die Linien trotz halber Dicke relativ breit. Im 1x-Crop treppen Bandkanten und Linien.
- **Proxy- oder Blender-bedingt, nicht dem Look anzulasten:** Die konvexen Proxies geben der invertierten Hülle saubere Silhouetten; bei Konkavitäten und Stofffalten zeigt die Technik typische Lücken und Durchstoßungen. Die Bänder wirken auf die Summe aller Lichter (Eevee-Einschränkung). Die im Design vorgesehene Quantisierung pro Licht hätte konzentrische Ringe je Fackel ergeben.
- **Technik:** Die Outlines sind eine invertierte Hülle (Solidify nach außen, gespiegelte Normalen, Backface-Culling, fast schwarzes #0B0A0D). Das ist die produktionsnahe Variante; ihre Kosten wachsen mit der Zahl und Dichte der Meshes, nicht mit der Auflösung.

### Stilisiertes 3D (weiches Licht, Rim, ohne Outlines)

- **Liest sich gut:** Weiches Licht ohne Bänder und ein Glanz auf Platten und Bronze. Der lichtunabhängige Rim trennt Spieler (Cyan) und Gegner (warm) auch in dunklen Bereichen. Das Gesamtbild wirkt am ehesten wie ein fertiges Spiel.
- **Schwach:** Auf Boden und Requisiten unterscheidet sich der Look kaum vom realistischen, weil Wrap und Schattentönung unter dem blau dominierten Mondlicht dezent bleiben. Die Unterscheidung kommt fast nur über den Rim der Figuren. Unter dichten Bullet-Wolken brennen die Cluster-Lichtpools hell in Bullet-Farbe aus, und der Bullet-Kontrast der unruhigen Variante fällt deutlich.
- **Proxy-bedingt:** Auf glatten konvexen Proxies wirkt der Fresnel-Rim besser, als er es auf detaillierten Modellen mit Konkavitäten tun wird.

### Realistisch (GGX, ohne Rim und Outlines)

- **Liest sich gut:** Glaubwürdige Lichtpools und Schattenwürfe. Metallischer Glanz an den Kohlebecken, Glanzpunkte auf den Schulterplatten.
- **Schwach:** Die Figuren wirken wie Plastik oder Knete. In dunklen Bereichen verschmelzen kleine Figuren mit dem Boden, und der Bullet-Kontrast ist ähnlich niedrig wie beim stilisierten Look.
- **Proxy-bedingt:** Ohne Texturen und Normal Maps ist dieser Look am stärksten benachteiligt. Echte GGX-Assets gewinnen am meisten durch Materialdetail.

### Look-übergreifend

- **Paletten-Risiko bestätigt:** Die Cluster-Lichter färben den Boden unter Bullet-Wolken in den Bullet-Farbton. Der dunkle Bullet-Rand hält die Silhouetten lesbar, aber der WCAG-Körperkontrast sinkt. Toon deckelt die Pools, die beiden weichen Looks nicht. Weniger Intensität oder Sättigung der Cluster-Lichter würde allen Looks helfen; das ist eine Frage des Licht-Designs, nicht des Looks.
- **Unterschiede zum Engine-Spike:** Blender rendert echte Shadow Maps für Mond, Fackeln und Stab-Orb sowie 32 temporale Samples mit Pixelfilter. Das hebt vor allem die beiden weichen Looks gegenüber dem Engine-Spike.
- **Renderzeiten:** Sie sind kein Kostenvergleich der Looks, nur ein Hinweis auf die Größenordnung in einem ausgereiften Renderer.
