# Glosc App Logo

The approved shared-stroke GA design is used without changing its geometry.

- `glosc-ga.png`: dark mark on a transparent background, for light themes.
- `glosc-ga-white.png`: white mark on a transparent background, for dark themes.
- `../../app-icon.png`: 1024px desktop icon master with a neutral light tile.
- `../../public/favicon.png`: 64px browser icon.
- `../../public/apple-touch-icon.png`: 180px opaque touch icon.

Regenerate the platform icons from the repository root:

```sh
yarn icons
```

For iOS distribution, remove any alpha left by the icon generator using a Python
environment with Pillow installed:

```sh
python3 scripts/finalize-ios-icons.py
```

Tauri uses the generated PNG, ICNS, and ICO files in `src-tauri/icons/` through
the existing `bundle.icon` entries in `src-tauri/tauri.conf.json`.

Design source: `output/imagegen/glosc-mod-logo-v3/shared-ga/`.
