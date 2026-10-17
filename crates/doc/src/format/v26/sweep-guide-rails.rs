//! A `Sweep` and a `SurfaceSweep` can carry guide `rails` (one or two, each a `sketch` and its
//! `curves` in order) and `rail_fit` (`scale`, the default, or `stretch`): the profile follows
//! the rails instead of an orientation, twist and end scale (#771). A sweep without them saves as
//! it always did, so older designs need no transformation; 0.13.x asks for an update instead of
//! sweeping a railed profile without its rails.
