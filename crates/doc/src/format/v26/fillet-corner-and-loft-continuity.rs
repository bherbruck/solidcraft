//! The Class-A surface pass also lets a `Fillet` carry `corner` (`rolling`, the default and what
//! every older design meant, or `g2`: a patch repeating each blend's surface up to the second
//! order) and a `SurfaceLoft` carry `continuity` and `crown` and body edge-chain `sections` (a
//! section picked as a `body` with `edges` instead of a `sketch`). A design without them saves as
//! it always did — a loft without `continuity` reads and writes as `[g0, g0]`, the plain loft it
//! always was — so older designs need no transformation; 0.13.x asks for an update instead of
//! meeting a feature it can't read or dropping a corner style it does not round.
