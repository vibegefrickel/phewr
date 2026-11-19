# The shared picture pipeline

The picture module is `tuilibr::pictures`. The plan and the rules live in `../tuilibr/docs/plan.md` and `../tuilibr/docs/pictures.md`. Change the module in tuilibr, not in phewr.

## History

This paragraph uses the old phase numbers of the picture plan. tuilibr phase 3 and tuilibr phase 4a are phases of the tuilibr plan. Phase 0 (2026-11-11) took the bench numbers of phewr. Phase 1 (2026-11-09, `6b7482a`) put the module in `src/pictures/` of phewr as pure code motion. Phase 2 (2026-11-09, `36ecdea`) made one Kitty layer. Phase 3 (2026-11-09, `688dc4e`) moved the shapes and the caches. Phase 4 (2026-11-09, schnattr `8f45abc`) made schnattr take the module. Phase 4b (2026-11-09, `455ac6f`) added the `kitty-*` features. Phase 5 (2026-11-11, `caa87b3`) added the self check of the Kitty placements, with 0 lost cells in herdr after ten scrolls. Phase 6 (2026-11-11, youtr `a5d338e`) made youtr take the module. Phase 7 is the test matrix of the maintainer. youtr passed it in Ghostty and in Ghostty inside herdr (2026-11-16). phewr and schnattr are open, and the maintainer moved them to later on 2026-11-19. The sync recipes `just pictures-sync` and `just pictures-check` copied the module until tuilibr phase 3 (2026-11-19). In tuilibr phase 3, the module moved into the crate tuilibr. phewr holds no copy, and the `kitty-*` features are gone. tuilibr phase 4a made the status row and the help come from the tuilibr frame. `main` calls `tuilibr::pictures::names::set_app_name` first.

## Open facts until the handover

tuilibr does not hold the facts below yet. They stay here until the plan of tuilibr takes them over.

### Phase 7: the test matrix

Each app, in each terminal, with each scenario:

- Terminals: Ghostty, Ghostty inside herdr, Kitty, WezTerm, Terminal.app (text only), SSH to the
  same Mac.
- Scenarios: the first start, ten scrolls up and down, a resize, a focus change to another app and
  back, the help overlay, a chat or folder with more than 500 pictures, the quit.
- Measures: the numbers of phase 0. The plan is done when no picture goes away, and each measure
  is equal to or better than the baseline.

The maintainer runs the scenarios that need a real terminal. The workers write the rest as tests.

Results:

- youtr (2026-11-16, youtr M14, the module copy of youtr `c7f2ece`): every scenario passed in
  Ghostty and in Ghostty inside herdr. The scenario with more than 500 pictures was the `All`
  entry, scrolled to its end. The maintainer decided not to test Kitty and WezTerm in youtr.
  Terminal.app and SSH are not tested. The measures of phase 0 were not taken. Before the run,
  youtr dropped its own skip of the answer reader for a resize and a focus event, because
  `AnswerKeys` passes these events itself now.
- phewr and schnattr: open.

Notes for the module, from the youtr review of 2026-11-16. Neither note showed in the test
matrix. The maintainer closed both on 2026-11-17 without a code change.

- **A focus gain sends every picture again.** `Screen::focus_gained` marks every stored picture
  for a new transmit, also in Ghostty, which keeps them. Only youtr and schnattr call it: phewr
  enables no focus reports. The cost is one screen of pixels for each focus gain, 2.5 MB for a
  full schnattr screen (phase 4), less than one update of 4 MB. The alternative (with the self
  check, a focus gain sends only the placements, and each `ENOENT` resends one picture) is
  correct only if herdr answers `ENOENT` for a picture that it dropped without the focus.
  Nobody tested that. Decision: keep the full resend.
- **An error other than `ENOENT` hides a picture until it moves.** The screen keeps such a
  picture as placed, and the default placing policy sends nothing until the picture moves or
  the window gets a resize. The realistic trigger is `ENOSPC` from a full storage quota. The
  store of `Screen` keeps every app below `TERMINAL_IMAGE_BUDGET` (128 MB). Ghostty cannot
  trigger it: its quota is 320 MB per terminal surface, and above the quota it evicts pictures
  without a placement without a notice. Two apps in two herdr panes share one Ghostty surface
  and can reach 2 x 128 MB x 2 = 512 MB (herdr stores each image twice). That case ends as a
  later `ENOENT`, which the self check handles, not as `ENOSPC`. herdr's own storage limit is
  not known, so herdr can still answer `ENOSPC`. Decision: no retry. A future retry needs a cap of one per picture, never for `EINVAL`, or a
  full terminal gets the same transmit on every frame. If the budget grows above 160 MB, check
  this note again.
