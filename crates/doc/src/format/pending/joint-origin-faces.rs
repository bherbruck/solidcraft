//! A face-centre or circle-centre joint origin can carry `x`: what turns its frame about z (a
//! direction, `[x, y, z]`, or `{face, pick?}`: a face by its persistent name), and a
//! circle-centre one `end`: the name of the face across the rim at the end it takes, so a part
//! stays on the same side of a hole when the design changes. A named face is now resolved by its
//! own plane or cylinder rather than the face nearest the pick. Older designs need no
//! transformation: an origin without `end` takes the end nearer its pick, and one without `x`
//! any direction square to z, as before; 0.14.x asks for an update instead of dropping a saved
//! end or turn.
