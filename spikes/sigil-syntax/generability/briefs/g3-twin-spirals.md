# G3 – Zwillingsspiralen mit Tempokurve

Schreibe ein Sigil-Pattern mit dem Namen „Twin Spirals“, Schwierigkeiten „hard“ und „curtain“,
geschätzte Dichte 400 Bullets.

Zwei Bullet-Sorten:

- `petal`: Silhouette `petal`, Palette `enemy.rose`, Glow 0.5, Radius 0.18 Welteinheiten, Schaden 1,
  grazebar. Wenn der Spieler pariert (Ereignis `player_parry`), kehren die Blütenblätter ihre
  Richtung um.
- `wisp`: Silhouette `crescent`, Palette `enemy.violet`, Glow 0.9, Radius 0.2, Schaden 1, grazebar,
  Bewegung über das registrierte Behaviour `seek_target_weak`.

Zwei Emitter mit Blütenblättern, beide ohne Ende wiederholt, alle 3 Ticks, Grundgeschwindigkeit
0.08 Welteinheiten pro Tick:

- `spiral_left` sitzt 2 Welteinheiten links vom Besitzer (x = −2, y = 0), feuert eine zweiarmige
  Spirale, die pro Schuss um 9 Grad weiterdreht, Start bei 90 Grad.
- `spiral_right` sitzt 2 Welteinheiten rechts (x = 2, y = 0), ebenfalls zweiarmig, dreht aber pro
  Schuss um −9 Grad, Start bei 90 Grad.

Beide Spiralen haben dieselbe Tempokurve über das Bullet-Alter mit linearer Interpolation: bei
0 Ticks Faktor 0.5, bei 30 Ticks Faktor 1.4, bei 80 Ticks Faktor 1.0.

Ein dritter Emitter `wisps` feuert nach 120 Ticks Verzögerung alle 60 Ticks, insgesamt fünfmal,
2 gezielte Irrlichter mit 20 Grad Streuung und 0.05 Welteinheiten pro Tick.
