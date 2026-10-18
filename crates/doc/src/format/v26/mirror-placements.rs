//! A Mirror of bodies keeps the placement each body was picked at (`placements`: one occurrence
//! path per body), and mirrors the body where that placement shows it, so a body of a moved or
//! inserted component, or of one of its further instances, is mirrored where it is seen. The copy
//! joins the body's component (or, when that is placed more than once, the innermost component
//! around it placed once). Older designs need no transformation: a Mirror without `placements`
//! mirrors the bodies' own geometry into its own component as before; the last release asks for
//! an update instead of mirroring them there.
