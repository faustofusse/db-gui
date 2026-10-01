# DBGui

Native database client. macOS first (SwiftUI), Linux later.

## Layout

```
Sources/
  DBCore/      # shared, UI-free: models, DatabaseDriver protocol, drivers (mock for now)
  DBGuiMac/    # SwiftUI app (macOS only), Mail-style 3-column NavigationSplitView
Tests/DBCoreTests
scripts/bundle-mac.sh   # builds + wraps into build/DBGui.app
```

`DBCore` must not import AppKit/SwiftUI so it keeps building on Linux.
Every frontend talks to databases only through `DatabaseDriver`.

## Run

```sh
swift test                      # core tests (also works on Linux)
./scripts/bundle-mac.sh && open build/DBGui.app
```

You can also open `Package.swift` in Xcode and run the `DBGuiMac` scheme.
