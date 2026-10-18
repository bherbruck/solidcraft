//! A component can carry its own part properties: `metadata` (`part_number`, `description`,
//! `revision`… and other tools' reverse-DNS keys, under the rules of the design's own `metadata`)
//! and `properties` (free text, by name), which every occurrence shares and the bill of materials
//! (`assembly.bom`) reads. Older designs need no transformation: a component without them has none.
//! The last release asks for an update instead of dropping them on save.
