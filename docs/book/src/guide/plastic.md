# Plastic parts

MODEL → CREATE lists the plastic enclosure features:

- **Boss** (`plastic.boss`): screw bosses with a hole, fillets and optional ribs.
- **Lip / Groove** (`plastic.lip`): the mating lip and groove along a shell's opening.
- **Snap Fit** (`plastic.snap_fit`): a cantilever snap hook and its catch.
- **Rest** (`plastic.rest`): a pad for a part to rest on.
- **Plastic Rules** (`plastic.manage_rules`, `plastic.assign_rule`): wall thickness, draft and
  clearance per material, used as the features' defaults.

Plastic Rules are in MANAGE → RULES. Rib and web in MODEL → CREATE complete the set. These features are checked against
hand-computed geometry.
