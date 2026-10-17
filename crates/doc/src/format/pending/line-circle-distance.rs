//! A sketch can hold a `line_circle_distance` dimension (#1803): the distance from a line `l` to
//! the edge of a circle or arc `c`, saved with its `value` and, when the line cuts the circle,
//! `inside` (the value is then how far the edge reaches past the line). Older designs need no
//! transformation: they have no such dimension; 0.15.x (which reads up to 26) asks for an update
//! instead of meeting a constraint type it doesn't know.
