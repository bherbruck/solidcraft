//! A design's `visibility` can also keep the rest of the browser's view of it: the Origin and
//! Sketches folders' eyes (`origin_hidden`, `sketches_hidden`), finished sketches whose profiles
//! are not shaded or whose dimensions are shown (`hidden_profiles`, `shown_dimensions`, sketch
//! ids) and locked bodies (`locked_bodies`, body names). Older designs need no transformation:
//! without them the folders are shown, profiles shaded, dimensions hidden and nothing is locked,
//! as before, and the Origin follows the Hide Origin by Default preference until the design
//! records its own. 0.13.x asks for an update; it would ignore them and lose only that view.
