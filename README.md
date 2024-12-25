# airpodsd
A daemon (for Linux only) that exposes AirPods battery information to `bluez` through its [`org.bluez.BatteryProviderManager`](https://github.com/bluez/bluez/blob/master/doc/org.bluez.BatteryProviderManager.rst) D-Bus API. It also keeps track of noise cancellation and in-ear status.

Currently, the left bud and right bud battery levels are averaged and only reported to `bluez` if both battery levels are available. It will be possible to configure this in the future.

`upower` based programs can also read this battery information as `upower` has a `bluez` backend for exposing battery information of Bluetooth devices.
As a result, AirPods battery information is shown just like any other Bluetooth device in the default system areas and is available to other power management utilities.

## Usage
Run `airpodsd daemon <mac_address>` in the background.

You can query the information that airpodsd has with `airpodsd status <mac_address>`.
This will automatically connect to a running airpodsd instance for that MAC address.

In the future, support for changing noise cancellation and in-ear status will be added.
