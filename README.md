# What is this?
This is a simple screen recorder written in Rust for Wayland Linux Desktop Environments.

It uses Nvidia's NVENC to encode the frames using the GPU, Opus as the audio encoder and the end goal
is to have this behave similar to applications like Medal.tv or Nvidia's Shadowplay feature where this
runs in the background while you play and you can use a keybind to clip the last [X] seconds of gameple.

Currently it offers video and audio capture when ran and exports the capture into an mp4 file all using ffmpeg.

Use `busctl --user call com.rust.GameClip /com/rust/GameClip com.rust.GameClip SaveClip` to invoke the same command.

# Core features
- [x] Asks permission from user to record their screen (Wayland limitation).
- [x] Captures video using pipewire.
- [x] Captures audio using pipewire.
- [x] On dbus command, saves what it has captures to a .mp4 file.
- [x] Uses GPU to encode frames to reduce CPU overhead. (Currently only supports nvidia) 
- [ ] Automatic game detection.
- [ ] Front end GUI for customizing settings.

### Known bugs I plan to fix
1. Video seems to sometimes be jittery likely due to variable frame rate and how PTS is set up.
2. Audio syncing logic could use some work, it's pretty rudimentary right now, unsure how it behaves in long sessions yet.
3. Game detection by using something like [procfs](https://crates.io/crates/procfs) and a known list of games.
4. This should be a background daemon that should auto start with systemd.
5. Front end GUI for modifying some settings like what [X] is.
