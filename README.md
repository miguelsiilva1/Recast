# Recast

All-to-all file converter for macOS, Windows and Linux. Everything runs locally through FFmpeg: no uploads, no limits.

- **Images:** PNG, JPG, WebP, AVIF, HEIC, TIFF, BMP → JPG, PNG, WebP*, AVIF, GIF, BMP, TIFF
- **Video:** MP4, MOV, MKV, WebM, AVI… → MP4, MOV, MKV, WebM, AVI, GIF, animated WebP*, audio, first-frame image
- **GIFs:** → MP4, WebM, MOV, WebP*, PNG/JPG
- **Audio:** MP3, WAV, FLAC, M4A, OGG, OPUS… → MP3, M4A, WAV, FLAC, OPUS, OGG*, AIFF

\* Only offered when the FFmpeg build includes the encoder (the app checks at startup).

## Develop

Requires Node.js, Rust and FFmpeg (`brew install ffmpeg`).

```sh
npm install
npm run tauri dev     # run the app
npm run tauri build   # build an installer for this OS
cd src-tauri && cargo test   # converts samples to every supported format
```

FFmpeg lookup order: bundled sidecar next to the executable → Homebrew/system locations → `PATH`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
