# Rust API to the ScreenCast Desktop Portal

Access to system resources on Linux can be negotitated in some environments
through calls to D-Bus 'portal' APIs. One such portal is the [`ScreenCast`][sc]
portal. This portal allows a user to choose which windows or screens to share
and provides access to raw video data through PipeWire.

## Simple Use

In the simples case this crate can be used to open a new screen cast with the
default settings:

```rust
let screen_cast = ScreenCast::new()?.Start()?;
```

## Structure

There are three main objects to interact with: `ScreenCast`, `ActiveScreenCast`,
and `ScreenCastStream`. The `ScreenCast` type is used to configure what type
of screen cast to prompt the user for. It is tramsformed into an
`ActiveScreenCast` by calling `start()`. Once active interaction with the cast
takes place over a Pipewire session using the `pipewire_fd()` and `streams()`.

Under the hood this is be backed by some private structs: `ConnectionState` to
manage our D-Bus connection; `Request`, and `Session` to handle interacting with
request and session proxies.

 [sc]: https://flatpak.github.io/xdg-desktop-portal/portal-docs.html#gdbus-org.freedesktop.portal.ScreenCast