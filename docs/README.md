# Design notes

These pages hold the design facts that the module docs no longer hold. Read AGENTS.md first.

- [Video and animations](video.md): the terminal player, the probe, the color cubes, the start ahead of Enter, the animated images, the grid loop frames and the mpv window.
- [Images and colors](images-and-colors.md): the decoders, libheif, the embedded JPEG, sips, the color conversion, the HDR stills and JPEG XL.
- [Grid and Kitty output](grid-and-kitty.md): the grid geometry, the cell shape, the Kitty writes, the terminal storage, the emit passes and the run loop waits.
- [Benchmarks](benchmarks.md): what the benches of `src/bench/` measure and what they leave out.
- [App state](app-state.md): the thumbnail jobs, the disk cache, the folder prefetch, the grid loop jobs and the fullscreen view.
- [Live Photos](live-photos.md): the pairing of an iPhone still and its `.mov`, the identifiers, the scan cost, the choices and the open questions.
- [Gotchas in detail](gotchas.md): the long form of each gotcha in AGENTS.md, with the numbers and the dates of the decisions.
- [Module map](layout.md): every file of `src/` with its main types and functions.
- [Tests in detail](testing.md): the `PHEWR_TEST_*` variables, the fixtures, their make scripts and the real-file tests.
