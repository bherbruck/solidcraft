//! A Hole placed by position can keep that position as length expressions (`at`: `[x, y, z]`,
//! numbers as numbers, expressions as strings), and is drilled where they evaluate, so it follows
//! the design's parameters (#1678). `position` still holds the point it was made at. Older designs
//! need no transformation: a hole without `at` is drilled at `position` as before; 0.13.x asks for
//! an update instead of drilling a moved hole at its old point.
