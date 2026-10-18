//! A standard part inserted from a part library (`parts.library.load`, a `solidcraft-parts/1`
//! file) keeps a copy of its fastener family in its `parts` entry (`definition`), so it rebuilds
//! and resizes without the library file, and the article number of its size and length
//! (`article`). Older designs need no transformation: their parts are built-in families and
//! have neither; 0.14.x asks for an update instead of opening a design that has one.
