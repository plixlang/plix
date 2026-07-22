# Plix Linux assets

The PNG files in `icons/` are generated from the original JPEG logo and use a
transparent background. Debian packages install them in the standard hicolor
icon theme under `/usr/share/icons/hicolor/<size>x<size>/apps/plix.png`.

The AppStream metadata file is installed as:

`/usr/share/metainfo/io.github.plixlang.plix.metainfo.xml`

Plix is a command-line application, so it intentionally does not install a
`.desktop` launcher.

The original source image is retained in `source/plix-logo-source.jpg`.
