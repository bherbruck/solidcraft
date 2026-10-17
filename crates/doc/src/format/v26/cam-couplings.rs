//! A motion link can carry a `cam` coupling (`joint.cam`): the cam's `profile` (a sketch's closed
//! profile by its outer loop's curve ids, or picked edges of the cam's bodies with their names),
//! the `follower` (`knife`, `roller` with a `radius`, or `flat`), an optional `tip` (a joint origin
//! where the follower touches) and `reverse` (the side it rides from). The link then also saves
//! `lift`, the follower's values over one turn of the cam, worked out from the profile again
//! whenever the design changes. Older designs need no transformation: a link without them is a
//! plain motion link, as it always was; 0.13.x asks for an update instead of reading a cam's
//! follower as a plain link of ratio 1.
