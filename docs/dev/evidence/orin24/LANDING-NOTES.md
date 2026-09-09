# orin 24 landing notes (rules recorded from peer messages; apply at the first fold that meets both tracks)
- unaos/scripts/ledger-check.sh and unaos/arch-families.ledger: resolve to hw-rmbp's version WHOLE; gate by symbol count (FIELDCOUNT_REG 5, ABSENCE_PHRASE 2, DELETION TRIGGER 2). (rmbp 18, 2026-09-09)
- unaos/arroyo: resolve to the version that has BOTH rmbp's verb table (knoboff <feature> [baseline-ref]) and orin's geometry (esp_jetson body, dispatch arm, usage token esp-jetson-img). Usage-string fix: `sed -i 's/|esp-jetson|/|esp-jetson|esp-jetson-img|/' unaos/arroyo` on rmbp's line; gate by `./arroyo` usage listing esp-jetson-img AND knoboff. GRANT for arroyo (UNAFSGROW shape only) recorded both transcripts 2026-09-09. (rmbp 18)
- S34 -> SO25 in the landing commit; S33 -> SR7 at rmbp's fold; freeze enforcer owed on hw-rmbp.
- main.rs / drivers/block.rs diffs from exec-orin24-unafsroot go to rmbp 18 before any merge.
