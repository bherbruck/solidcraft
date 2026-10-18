//! A `SurfaceTrim` can keep several regions (`keep_more`: more points; with them, the pieces that
//! hang together once cut that `keep` and these points pick are kept instead of the side of
//! `keep`), cut with sketch curves swept onto the surface (`curves`: `sketch`, optional `curves`
//! ids and `direction`), or trim another surface and be trimmed by it (`with`, `keep_with` points,
//! optional `stitch` and `extend`). Older designs need no transformation: a trim without them
//! saves as before; 0.14.x asks for an update instead of ignoring them and trimming by the plane.
