```ron
#![enable(implicit_some)]
Sigil(
    version: 1,
    meta: Meta(
        name: "Twin Spirals",
        difficulty: ["hard", "curtain"],
        density: 400,
    ),
    bullets: [
        Bullet(
            name: "petal",
            silhouette: "petal",
            palette: "enemy.rose",
            glow: 0.5,
            radius: Units(0.18),
            damage: 1,
            flags: [Grazeable],
            transforms: [
                Reverse(when: Event("player_parry")),
            ],
        ),
        Bullet(
            name: "wisp",
            silhouette: "crescent",
            palette: "enemy.violet",
            glow: 0.9,
            radius: Units(0.2),
            damage: 1,
            flags: [Grazeable],
            behaviour: "seek_target_weak",
        ),
    ],
    emitters: [
        Emitter(
            name: "spiral_left",
            bullet: "petal",
            offset: (x: Units(-2.0), y: Units(0.0)),
            repeat: Forever,
            interval: Ticks(3),
            speed: UnitsPerTick(0.08),
            block: Spiral(
                arms: 2,
                step: Deg(9.0),
                start: Deg(90.0),
            ),
            modifiers: [
                SpeedCurve(
                    keys: [
                        (at: Ticks(0), mul: 0.5),
                        (at: Ticks(30), mul: 1.4),
                        (at: Ticks(80), mul: 1.0),
                    ],
                    interp: Linear,
                ),
            ],
        ),
        Emitter(
            name: "spiral_right",
            bullet: "petal",
            offset: (x: Units(2.0), y: Units(0.0)),
            repeat: Forever,
            interval: Ticks(3),
            speed: UnitsPerTick(0.08),
            block: Spiral(
                arms: 2,
                step: Deg(-9.0),
                start: Deg(90.0),
            ),
            modifiers: [
                SpeedCurve(
                    keys: [
                        (at: Ticks(0), mul: 0.5),
                        (at: Ticks(30), mul: 1.4),
                        (at: Ticks(80), mul: 1.0),
                    ],
                    interp: Linear,
                ),
            ],
        ),
        Emitter(
            name: "wisps",
            bullet: "wisp",
            delay: Ticks(120),
            repeat: Times(5),
            interval: Ticks(60),
            speed: UnitsPerTick(0.05),
            block: Aimed(
                count: 2,
                spread: Deg(20.0),
            ),
        ),
    ],
)
```

```sigil
sigil 1

meta {
  name = "Twin Spirals"
  difficulty = [hard, curtain]
  density = 400
}

bullet petal {
  silhouette = petal
  palette = enemy.rose
  glow = 0.5
  radius = 0.18u
  damage = 1
  flags = [grazeable]
  transform reverse {
    when = event player_parry
  }
}

bullet wisp {
  silhouette = crescent
  palette = enemy.violet
  glow = 0.9
  radius = 0.2u
  damage = 1
  flags = [grazeable]
  behaviour = seek_target_weak
}

emitter spiral_left {
  bullet = petal
  offset = (x = -2u, y = 0u)
  repeat = forever
  interval = 3t
  speed = 0.08u/t
  block spiral {
    arms = 2
    step = 9deg
    start = 90deg
  }
  modifier speed_curve {
    keys = [
      (at = 0t, mul = 0.5),
      (at = 30t, mul = 1.4),
      (at = 80t, mul = 1.0),
    ]
    interp = linear
  }
}

emitter spiral_right {
  bullet = petal
  offset = (x = 2u, y = 0u)
  repeat = forever
  interval = 3t
  speed = 0.08u/t
  block spiral {
    arms = 2
    step = -9deg
    start = 90deg
  }
  modifier speed_curve {
    keys = [
      (at = 0t, mul = 0.5),
      (at = 30t, mul = 1.4),
      (at = 80t, mul = 1.0),
    ]
    interp = linear
  }
}

emitter wisps {
  bullet = wisp
  delay = 120t
  repeat = 5
  interval = 60t
  speed = 0.05u/t
  block aimed {
    count = 2
    spread = 20deg
  }
}
```