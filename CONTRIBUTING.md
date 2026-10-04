# Contributing

Bug reports and pull requests are welcome.

## Bugs

Open an issue with: what you did, what you expected, what happened, your
`paddock --version`, `herdr --version`, OS and terminal. If a pane renders
wrong, say which program was running in it.

## Pull requests

- One change per PR. Say what problem it solves in the description.
- Keep the code in the style you find: small functions, few comments, no new
  dependencies without a reason.
- `cargo build` must produce no warnings and `cargo test` must pass.
- Run it against a live herdr before and after. Changes to pane rendering or
  input need a check inside a real pane, not only unit tests.
- Update README.md when behaviour or keys change.

## Scope

Paddock is a client for herdr. It reads and controls herdr through herdr's own
CLI and socket API and does not reimplement herdr features. Ideas that need
changes in herdr itself belong in a herdr Discussion.

## License

By contributing you agree that your contributions are licensed under the MIT
License in this repository.
