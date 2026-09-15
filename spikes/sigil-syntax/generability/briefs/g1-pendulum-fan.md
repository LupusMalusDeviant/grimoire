# G1 – Pendelfächer

Schreibe ein Sigil-Pattern mit dem Namen „Pendulum Fan“, Schwierigkeit „normal“, geschätzte
Dichte 90 Bullets.

Es gibt genau eine Bullet-Sorte namens `leaf`: Silhouette `leaf`, Palette `enemy.jade`, Glow 0.4,
Radius 0.15 Welteinheiten, Schaden 1, grazebar, sonst keine Flags.

Ein Emitter namens `pendulum` feuert diese Blätter als Fächer aus 7 Bullets, der sich über
insgesamt 90 Grad öffnet und zur Bildschirmunterkante zeigt (270 Grad). Er schießt zwölfmal, alle
10 Ticks, mit einer Grundgeschwindigkeit von 0.1 Welteinheiten pro Tick. Der Fächer dreht sich
langsam mit 1.5 Grad pro Tick, und jedes Blatt pendelt seitlich mit einer Sinus-Auslenkung von
0.5 Welteinheiten und einer Periode von 60 Ticks (Phase 0 Grad). Die Drehung wird vor der
Sinus-Auslenkung angewendet.
