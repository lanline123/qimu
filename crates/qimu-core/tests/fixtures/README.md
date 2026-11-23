# Test fixtures

`Case2_Charts_Animation.pptx` is generated, never committed (`*.pptx`
is gitignored repo-wide). Regenerate it with:

    QIMU_CASE2_OUT=crates/qimu-core/tests/fixtures/Case2_Charts_Animation.pptx \
      python3 tools/make_case2.py

Requires python-pptx, Pillow, and ffmpeg (ffmpeg produces the 7th,
video, slide that the tests assert on). The p0-deckcraft CI workflow
runs this automatically before the qimu-core unit tests.

The Redwood deck used by other qimu-core tests is a real-world deck
and is intentionally not kept here; those tests resolve it via the
`QIMU_REDWOOD_DECK` env var or the legacy `user/files/` location and
skip visibly when it is absent. See
`crates/qimu-core/src/test_fixtures.rs`.
