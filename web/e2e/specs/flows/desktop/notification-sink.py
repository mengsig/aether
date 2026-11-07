#!/usr/bin/env python3
# A freedesktop notification server on a private session bus, for the packaged desktop flows:
# it records every Notify as a JSON line, and Invoke(id, action) clicks a notification.
import json
import sys
from datetime import datetime, timezone

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

INTERFACE = "org.freedesktop.Notifications"
CONTROL = "org.aether.E2E"

assert len(sys.argv) == 3, "usage: notification-sink.py <bus address> <log>"
ADDRESS, LOG = sys.argv[1], sys.argv[2]
assert ADDRESS.startswith("unix:path=/"), "the sink serves a private unix bus only"
dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.bus.BusConnection(ADDRESS)
name = dbus.service.BusName(INTERFACE, bus=bus, do_not_queue=True)


class Notifications(dbus.service.Object):
    next_id = 1

    @dbus.service.method(INTERFACE, in_signature="", out_signature="as")
    def GetCapabilities(self):
        return ["body", "actions"]

    @dbus.service.method(INTERFACE, in_signature="", out_signature="ssss")
    def GetServerInformation(self):
        return ("Aether e2e sink", "Aether", "1", "1.2")

    @dbus.service.method(INTERFACE, in_signature="susssasa{sv}i", out_signature="u")
    def Notify(self, app, replaces, icon, title, body, actions, hints, timeout):
        assert int(replaces) >= 0 and int(timeout) >= -1, "Notify gets a valid id and timeout"
        ident = self.next_id
        self.next_id += 1
        entry = {"at": datetime.now(timezone.utc).isoformat(), "id": ident, "app": str(app), "title": str(title),
                 "body": str(body), "actions": [str(action) for action in actions]}
        with open(LOG, "a", encoding="utf-8") as file:
            file.write(json.dumps(entry) + "\n")
        return ident

    @dbus.service.method(INTERFACE, in_signature="u", out_signature="")
    def CloseNotification(self, ident):
        assert int(ident) > 0, "a notification id is positive"
        self.NotificationClosed(ident, 3)

    @dbus.service.method(CONTROL, in_signature="us", out_signature="")
    def Invoke(self, ident, action):
        assert 0 < int(ident) < self.next_id, "only a shown notification is clicked"
        self.ActionInvoked(ident, action)

    @dbus.service.signal(INTERFACE, signature="uu")
    def NotificationClosed(self, ident, reason):
        return None

    @dbus.service.signal(INTERFACE, signature="us")
    def ActionInvoked(self, ident, action):
        return None


service = Notifications(bus, "/org/freedesktop/Notifications")
print("ready", flush=True)
GLib.MainLoop().run()
