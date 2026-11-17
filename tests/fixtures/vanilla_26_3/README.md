# Vanilla 26.3 section fixtures

`*.bin`: one block-state section each, written by vanilla 26.3's `PalettedContainer.write` (block-state
strategy over an `IdMapper` of 30000 entries, so 15 global bits). `generator/Golden.java` wrote them;
`generator/Verify.java` reads sections back with `PalettedContainer.read` and checks every cell.

    CP=<26.3.jar>:<the game's library jars>
    javac -cp "$CP" -d out generator/Golden.java generator/Verify.java
    java -cp "$CP:out" Golden .                      # regenerate the fixtures
    AXIOM_WRITE_SECTIONS=/tmp/rs cargo test --lib write_sections_for_vanilla
    java -cp "$CP:out" Verify /tmp/rs                # vanilla reads what the port writes
