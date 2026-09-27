# Working in ARC KDL

This repository contains the KDL designs and KiCad projects for ARC boards. Use this file for collaboration and workflow. Keep system behavior in the root `README.md`, board behavior and decisions in each board's `README.md`, and circuit details in the KDL source.

## Read first

- `README.md` explains the project layout and how to check a design.
- Read the target board's `README.md`, `board.kdl`, and adjacent KDL files before editing it.
- Read `shared/` definitions before changing a circuit used by multiple boards.

## Working together

- Check `git status` and the relevant board directory before editing. Other agents may be working in the same checkout.
- Divide parallel work by board or shared file. Coordinate before editing a shared definition or file another agent owns; preserve unrelated edits and do not reset them.
- Keep a board's KDL, README, KiCad project, and `.stackup_sch` link consistent when moving or renaming files. Do not regenerate or discard PCB placement and routing as a side effect of a KDL edit.
- Run `stackup check` on each changed `board.kdl`. When a shared definition changes, check every board that imports it. Report the checks run and any layout work still needed.

The sibling `stackup` and `stackup-parts` repositories supply the tool and library. Coordinate changes that span their interfaces and this design set.
