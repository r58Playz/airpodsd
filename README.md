# airpodsd
A daemon (for Linux only) that exposes AirPods battery information to `bluez` through its [`org.bluez.BatteryProviderManager`](https://github.com/bluez/bluez/blob/master/doc/org.bluez.BatteryProviderManager.rst) D-Bus API. Now rewritten to support AirPods BLE advertisements as well!

If both the left and right bud battery levels are available, their average is reported by default. This behavior can be configured with `--battery-aggregation`, which supports `average`, `min`, and `max`.
If only one bud's battery level is available, it is reported directly. Otherwise, no battery level is reported at all.

`upower` based programs can also read this battery information as `upower` has a `bluez` backend for exposing battery information of Bluetooth devices.
As a result, AirPods battery information is shown just like any other Bluetooth device in the default system areas and is available to other power management utilities.

## Usage
Run `airpodsd daemon /path/to/keystore` in the background.

airpodsd will create the keystore file once it's needed to store BLE decryption keys.

## Usage with systemd
Copy `airpodsd.service` to `~/.config/systemd/user/` or the systemd user service location on your system.

Enable (and start) the service with `systemctl --user enable --now airpodsd`.

## Battery aggregation
The `--battery-aggregation` option controls how the left and right bud battery levels are combined into the single percentage reported to the system:

- `average` reports the average of both levels, rounded down to a whole percentage. This is the default when the option is omitted.
- `min` reports the lower of the two levels. Use it to monitor the weaker battery when you want to keep using both buds together.
- `max` reports the higher of the two levels. Use it to monitor the stronger battery when you want to keep using at least one bud.

For example, to report the lower battery level:

**CLI:**

```sh
airpodsd daemon /path/to/keystore --battery-aggregation min
```

**systemd:** Update the `ExecStart` line in your user service file, then run `systemctl --user daemon-reload` and `systemctl --user restart airpodsd`:

```ini
ExecStart=%h/.cargo/bin/airpodsd daemon "%h/.config/airpodsd_keystore" --battery-aggregation min
```
