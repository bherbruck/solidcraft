//! A Rib or Web can grow along the sketch normal (`normal: true`): the wall is `thickness` wide
//! across its curves in the sketch plane and runs from the sketch toward the body. Older designs
//! need no transformation: a rib without `normal` grows across the sketch plane as before; 0.14.x
//! asks for an update instead of building a normal rib in the plane.
