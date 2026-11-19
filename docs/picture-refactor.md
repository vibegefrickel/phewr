# The shared picture pipeline

The picture module is `tuilibr::pictures`. The plan and the rules live in `../tuilibr/docs/plan.md` and `../tuilibr/docs/pictures.md`. Both paths need the clone of tuilibr next to phewr, like the build. Change the module in tuilibr, not in phewr.

## History

This paragraph uses the old phase numbers of the picture plan. tuilibr phase 3 and tuilibr phase 4a are phases of the tuilibr plan. Phase 0 (2026-11-11) took the bench numbers of phewr. Phase 1 (2026-11-09, `6b7482a`) put the module in `src/pictures/` of phewr as pure code motion. Phase 2 (2026-11-09, `36ecdea`) made one Kitty layer. Phase 3 (2026-11-09, `688dc4e`) moved the shapes and the caches. Phase 4 (2026-11-09, schnattr `8f45abc`) made schnattr take the module. Phase 4b (2026-11-09, `455ac6f`) added the `kitty-*` features. Phase 5 (2026-11-11, `caa87b3`) added the self check of the Kitty placements, with 0 lost cells in herdr after ten scrolls. Phase 6 (2026-11-11, youtr `a5d338e`) made youtr take the module. Phase 7 is the test matrix of the maintainer. youtr passed it in Ghostty and in Ghostty inside herdr (2026-11-16). phewr and schnattr are open, and the maintainer moved them to later on 2026-11-19. The sync recipes `just pictures-sync` and `just pictures-check` copied the module until tuilibr phase 3 (2026-11-19). In tuilibr phase 3, the module moved into the crate tuilibr. phewr holds no copy, and the `kitty-*` features are gone. tuilibr phase 4a made the status row and the help come from the tuilibr frame. `main` calls `tuilibr::pictures::names::set_app_name` first.

The open facts of the test matrix and the two decisions of 2026-11-17 moved into
`../tuilibr/docs/pictures.md`, section "The handover from phewr (2026-11-19)", on 2026-11-19.
