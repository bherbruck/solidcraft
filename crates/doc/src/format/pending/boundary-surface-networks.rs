//! A `SurfaceNetwork` (Boundary Surface through a curve network, #763) saves `u` and `v`, each a
//! list of curves: a `sketch` and its `curves`, a `body` and points on its `edges`, or a `point`
//! (the apex of a three-sided patch), with an optional `continuity` (`g1` or `g2` with the face
//! beside an edge chain), and an `operation` (`new_body`, or `join` into the body of its edges).
//! A `SurfaceBoundary` of three or four curves, which older builds refused, now builds and saves
//! as it always did. Older designs need no transformation; the last release asks for an update
//! instead of meeting a feature it can't read.
