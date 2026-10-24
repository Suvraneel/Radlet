**Radlet is a weapon wheel for your files.** Drag files in Finder, hold a key, and flick to a tool: convert, compress, crop, translate and dozens more, all on your Mac.

**Install:** download the `.dmg` below, open it and drag **Radlet** into **Applications**. Radlet runs on macOS 14 or later, on Apple silicon and Intel Macs.

Radlet isn't notarized by Apple yet, so macOS blocks it the first time you open it:

- **macOS 15 or later:** click **Done**, then open **System Settings → Privacy & Security**, find "Radlet was blocked to protect your Mac", click **Open Anyway** and confirm with Touch ID or your password.
- **macOS 14:** Control-click Radlet in Applications, choose **Open**, then click **Open** again.

For audio and video, install FFmpeg: `brew install ffmpeg`. Images, PDFs and text work without it.

**Getting started:** Radlet lives in the menu bar. Start dragging files in Finder and hold **Shift** to convert, **Command** for tools or **Control** for your loadouts. No drag needed: select files in Finder and press **⌃⌥V**. The README has the full guide.

**Coming from a Veer build?** Radlet carries your settings, loadouts (formerly recipes), skins and History over when it starts. Reinstall the command-line tool from **Settings → Advanced** (it's now `radlet`), turn **Start at login** back on, then delete `Veer.app`.
