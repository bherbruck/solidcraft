# Analytic reference values

Closed-form expected values for the cases in [`tests/oracle/`](../../tests/oracle/) whose shape
has one: prisms, cylinders, cones and frustums, spheres and caps, tori and solids of revolution
(Pappus), fillets on straight edges (a constant radius, or one varying along the edge), shells,
patterns of these, and folded sheet metal (flats plus bends as annular sectors). Each script is
plain Python 3 (standard library only) with one function per case. Running a script from the repository root rewrites the
`expected.json` of its cases with `"source": "analytic"` and a `method` string giving the formulas:

```sh
python3 tools/oracle-analytic/cases_basic.py        # 01-29, 82: boxes, holes, revolves, sketches, thin walls
python3 tools/oracle-analytic/cases_features.py     # 32-66: fillets, shells, sweeps, patterns
python3 tools/oracle-analytic/cases_sheet_metal.py  # 67-76: sheet metal
python3 tools/oracle-analytic/cases_fillets.py      # 77-81: variable, chord-length, tapered and spline fillets
python3 tools/oracle-analytic/cases_surfaces.py     # 83: boundary surfaces
```

Where a value depends on a SolidCraft convention rather than on geometry alone (a shell's sharp
inner corner, a sheet-metal corner relief, where a flange height is measured), `method` or
`note` says which convention is used. Never change a value to match SolidCraft's output: if they
disagree, find out which one is wrong.
