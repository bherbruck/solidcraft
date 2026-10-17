# Parameters and expressions

Every dimension and feature input is a **model parameter** (`d1`, `d2`, …). **User parameters**
are ones you add yourself, with a unit, an expression and an optional comment. Change them in
**Change Parameters** (`parameters.change`). The timeline rebuilds from the first feature that
uses them.

In the SOLID toolbar's CONFIGURE panel, click **Parameters** to open the user and model
parameter editor. The separate **Configurations** button opens the configuration table;
**Configure** remains in the dropdown for configuring columns.

Press **Enter** in the new parameter's name, expression or comment field to add it.
Valid entries return focus to the name field for the next parameter; invalid entries remain
editable. In expression fields, type a parameter or dimension name for autocomplete,
or press **Ctrl+Space** to open suggestions, including in an empty field.
**Up/Down** selects an autocomplete suggestion,
**Enter/Tab** accepts it, and **Escape** dismisses the suggestions. Accepting a suggestion
does not also submit the parameter.

Expressions are unit-aware: `2 * width + 5 mm`, `angle / 2`, `sqrt(area)`, `1 in + 3 mm`, using
`+ - * / ^`, parentheses, `sin cos tan asin acos atan atan2 sqrt abs min max floor ceil round mod
trunc` and unit conversion. A decimal comma works too, outside parentheses: `2,5 mm` is
`2.5 mm` (inside a function's parentheses the comma separates its arguments, so write
`max(2.5, 3)` there). A mixed number is one value, as imperial sizes are written: `2 1/8"` and
`3 7/16 in` are 2.125 in and 3.4375 in (a whole number, a space, then a proper fraction of whole
numbers). A length where an angle is expected (or the other way round) is an
error, not a silent conversion. Cycles are reported with their path (`a → b → a`), renaming a
parameter updates every expression, dimension and feature input that uses it, and a parameter
in use can't be deleted (the error lists its users).

The design's units (Document Settings in the browser: mm, cm, m, in or ft) set how a bare number
typed into a feature is read. In an inch design `2` means 2 in. The Box, Cylinder, Sphere, Torus
and Coil dialogs start with their sizes in the design's units, unit shown (`20 mm`, `20 in`).
Changing the units never moves existing geometry. A size worked out from a bare number keeps
the unit the number was given in: a sphere with a bare diameter of 20 has the radius
`(20 mm) / 2` in a millimetre design. Beside a unit-less expression such as `2 * 5` or a
unit-less parameter, which is read in the design's units, the number stays bare and is read
with it.

Parameters export to and import from CSV or JSON (`parameters.export`, `parameters.import`).
**Configurations** (`config.*`) keep a table of rows, each with its own parameter values and
suppressed features, physical materials and body appearances, switched with `config.activate`.
The table's Library picker selects material or appearance values. Choose **Follow material**
to clear an appearance override. Custom looks retain their colour and opacity across
activation, undo and save/reopen. See
[configuration commands and examples](https://github.com/bherbruck/solidcraft/blob/main/docs/configurations.md).
