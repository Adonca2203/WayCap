# What is this?
This is a simple screen recorder written in Rust for Wayland Linux Desktop Environments.

It uses hardware acceleration to encode the video frames using the GPU, Opus as the audio encoder and the end goal
is to have this behave similar to applications like Medal.tv or Nvidia's Shadowplay feature where this
runs in the background while you play and you can use a keybind to clip the last [X] seconds of gameplay.

Currently it offers video and audio capture when ran and exports the capture into an mp4 file all using ffmpeg.

Use `busctl --user call com.rust.WayCap /com/rust/WayCap com.rust.WayCap SaveClip` to invoke the save command.

# Core features
- [x] Asks permission from user to record their screen (Wayland limitation).
- [x] Captures video using pipewire.
- [x] Captures audio using pipewire.
- [x] On dbus command, saves what it has captures to a .mp4 file.
- [x] Uses GPU to encode frames to reduce CPU overhead. (Supports NVIDIA with h264.nvenc and ADM with h264.vaapi) 
- [x] Customize certain options via an user level config file in ~/.config
- [ ] Automatic game detection.
- [ ] Front end GUI for customizing settings.

### Known bugs/things I plan to implement
1. Game detection by using something like [procfs](https://crates.io/crates/procfs) and a known list of games.
2. This should be a background daemon that should auto start with systemd.
3. Front end GUI for settings
4. Normal recording/manual execution can stay instead of only for games?

### Notes
By default this will try to use h264_vaapi as it can support AMD/Nvidia/Intel. It is recommended to use `nvenc`
if you have an nvidia GPU by updating the configuration file.

### Configuration
Currently, this program only supports configurations via a `config.toml` file in `~/.config/waycap/`

The application will automatically create a default one for you if it is not present. This is what it looks like
```toml
encoder = "h264_vaapi" # h264_nvenc | h264_vaapi
max_seconds = 300 # 5 minutes is the default but can be anything -- be aware this can have impact on performance as this grows bigger
use_mic = false # true | false
quality = "MEDIUM" # LOW | MEDIUM | HIGH | HIGHEST -- impacts file size and can impact performance
```
The comments are the available options.

### Minimum Requirement
- NVIDIA GPU with CUDA capabilities or AMD GPU with mesa drivers
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
busctl --user call com.rust.WayCap /com/rust/WayCap com.rust.WayCap SaveClip
```

Alternatively, bind the above busctl call to a keybind with something like [sxhkd](https://github.com/baskerville/sxhkd)

Find the moment in the clip you want and trim the video using the helper script
```
FILE_NAME=
START_TIME=
END_TIME=

./clip.sh -i $FILE_NAME -s $START_TIME -e $END_TIME -o output.mp4
```
