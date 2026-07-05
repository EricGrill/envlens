Output docs/demo.gif

Set Shell "zsh"
Set Width 1200
Set Height 700
Set FontSize 16
Set TypingSpeed 35ms

Type "cargo build --release"
Enter
Sleep 1s

Env PATH "target/release:$PATH"
Type "export PATH=\"$(pwd)/target/release:$PATH\""
Enter
Type "cd tests/fixtures/basic"
Enter
Type "envlens"
Enter
Sleep 1s

Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Enter
Sleep 300ms
Type "/"
Type "po"
Sleep 300ms
Escape
Sleep 250ms
Type "f"
Sleep 250ms
Type "f"
Sleep 250ms
Type "?"
Sleep 1s
Type "q"
