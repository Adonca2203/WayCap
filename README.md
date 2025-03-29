# What is this?
This is a simple screen recorder written in Rust for Wayland Linux Desktop Environments.

It uses Nvidia's NVENC to encode the frames using the GPU, Opus as the audio encoder and the end goal
is to have this behave similar to applications like Medal.tv or Nvidia's Shadowplay feature where this
runs in the background while you play and you can use a keybind to clip the last [X] seconds of gameplay.

Currently it offers video and audio capture when ran and exports the capture into an mp4 file all using ffmpeg.

Use `busctl --user call com.rust.GameClip /com/rust/GameClip com.rust.GameClip SaveClip` to invoke the save command.

# Core features
- [x] Asks permission from user to record their screen (Wayland limitation).
- [x] Captures video using pipewire.
- [x] Captures audio using pipewire.
- [x] On dbus command, saves what it has captures to a .mp4 file.
- [x] Uses GPU to encode frames to reduce CPU overhead. (Currently only supports nvidia) 
- [x] Customize certain options via an user level config file in ~/.config
- [ ] Automatic game detection.
- [ ] Front end GUI for customizing settings.

### Known bugs/things I plan to implement
1. Game detection by using something like [procfs](https://crates.io/crates/procfs) and a known list of games.
2. This should be a background daemon that should auto start with systemd.
3. Audio can go out of sync with the video and end up further ahead.

### Notes
This application currently supports GPU encoding via `h264_nvenc` and audio encoding via `opus` utilizing
ffmpeg.

Other video encoders may work but am unable to test on anything that is not NVIDIA. Feel free to change the encoder in
the config file under ~/.config/auto-screen-recorder

### Minimum Requirement
- NVIDIA GPU with CUDA capabilities recommended
- Wayland as your communication server for your desktop environment. (X11 planned but not priority)
- Rust/Cargo installation [link](https://www.rust-lang.org/tools/install) to build the project.

## Installation Guide
Clone this repository via
```
git clone https://github.com/Adonca2203/screen-recorder.git
```
Build the project (debug build) via
```
cd screen-recorder
cargo build
```

## Usage Guide
You can run the application as a debug build via
```
cargo run
```
from within your cloned project's directory.

The program will prompt you to select the screen you would like to share with the application, select the appropriate display option/

Play games and have fun

Whenever you want to clip, open up another terminal and run
```
busctl --user call com.rust.GameClip /com/rust/GameClip com.rust.GameClip SaveClip
```

Alternatively, bind the above busctl call to a keybind with something like [sxhkd](https://github.com/baskerville/sxhkd)

Find the moment in the clip you want and trim the video using the helper script
```
FILE_NAME=
START_TIME=
END_TIME=

./clip.sh -i $FILE_NAME -s $START_TIME -e $END_TIME -o output.mp4
```
