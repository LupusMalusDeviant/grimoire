# G2 – Minenfeld

Schreibe ein Sigil-Pattern mit dem Namen „Mine Field“, Schwierigkeiten „hard“ und „trap“,
geschätzte Dichte 200 Bullets, Patron `ember_court`.

Drei Bullet-Sorten:

- `mine`: Silhouette `orb`, Palette `enemy.crimson`, Glow 0.8, Radius 0.4 Welteinheiten, Schaden 2,
  zerschlagbar und grazebar. Nach 90 Ticks Lebenszeit platzt die Mine: Sie wird zu einem Ring aus
  8 Bullets der Sorte `splinter`, Startwinkel 0 Grad, Geschwindigkeit 0.12 Welteinheiten pro Tick.
- `splinter`: Silhouette `shard`, Palette `enemy.amber`, Glow 0.5, Radius 0.12, Schaden 1, grazebar.
  Nach 6 zurückgelegten Welteinheiten wechselt ein Splitter seinen Typ zu `cinder`.
- `cinder`: Silhouette `diamond`, Palette `enemy.amber`, Glow 0.2, Radius 0.12, Schaden 1, grazebar.
  Beim Bullet-Clear spielt es den Effekt `ash_puff` ab.

Ein Emitter `layer` legt die Minen: einmalig nach 30 Ticks Verzögerung eine Linie aus 5 Minen mit
1.2 Welteinheiten Abstand in Richtung 0 Grad, sehr langsam mit 0.02 Welteinheiten pro Tick. Das
Intervall ist 1 Tick.
