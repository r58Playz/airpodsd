# airpodsd
A daemon (for Linux only) that exposes AirPods battery information to `bluez` through its [`org.bluez.BatteryProviderManager`](https://github.com/bluez/bluez/blob/master/doc/org.bluez.BatteryProviderManager.rst) D-Bus API. Now rewritten to support AirPods BLE advertisements as well!

If both the left and right bud battery levels are available, the average of both buds is reported. If only one of the left or right bud battery levels are available, it is reported directly. Otherwise, no battery level is reported at all.

`upower` based programs can also read this battery information as `upower` has a `bluez` backend for exposing battery information of Bluetooth devices.
As a result, AirPods battery information is shown just like any other Bluetooth device in the default system areas and is available to other power management utilities.

## Usage
Run `airpodsd daemon /path/to/keystore` in the background.

airpodsd will create the keystore file once it's needed to store BLE decryption keys.

## Usage with systemd
Copy `airpodsd.service` to `~/.config/systemd/user/` or the systemd user service location on your system.

Enable (and start) the service with `systemctl --user enable --now airpodsd`.
