# Test fixtures

Nothing in this folder is committed that vanilla Minecraft generated. The vanilla 26.3 section fixtures
(`vanilla_26_3/*.bin`) are git-ignored and must never be committed.

Tests that need them read them from the directory in `AXIOM_VANILLA_FIXTURES` and skip otherwise.

## Regenerate locally

From your own Minecraft 26.3 client or server jar (plus its library jars), in `tests/fixtures/vanilla_26_3/`:

    CP=<26.3.jar>:<the game's library jars>
    cd tests/fixtures/vanilla_26_3
    javac -cp "$CP" -d /tmp/axiom-golden generator/Golden.java generator/Verify.java
    java -cp "$CP:/tmp/axiom-golden" Golden .          # writes the *.bin files next to the generator

Then run the test against them:

    AXIOM_VANILLA_FIXTURES=/abs/path/to/tests/fixtures/vanilla_26_3 cargo test --lib reads_the_sections_vanilla_26_3_writes

Never commit the generated `*.bin` files (git-ignored) or anything else derived from the jar.
