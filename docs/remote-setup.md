# Use Aether from your phone

This guide gets Aether onto your phone's home screen in a few minutes. You do
not need to know what Tailscale is, and you do not edit any files. One command
walks you through everything, and you can run it again at any time: it checks
what is already done and only fixes what is not.

What you will have at the end: an Aether icon on your phone that shows your
agents, lets you answer their questions and approve their requests, and
notifies you when one needs you. The agents keep running on your computer;
the phone is a window onto them.

For the protocol and every endpoint, see [remote control](remote.md).

## Before you start

- Aether installed on the computer where your agents run, with `aether doctor`
  passing.
- A phone (iPhone, iPad or Android) and about ten minutes.
- An account for Tailscale. If you have none, the guide below creates one with
  your Google, Microsoft, Apple or GitHub login. Personal use is free.

## The short version

On the computer that runs Aether, in a terminal:

```sh
aether remote setup
```

Answer the questions. When it shows a QR code, scan it with your phone's
camera. The rest of this page explains each step and what to do when one fails.

## What Tailscale is, and why Aether uses it

Tailscale gives your computer and your phone a small private network of their
own, which it calls a *tailnet*. Only devices signed in to **your** Tailscale
account are on it. Aether publishes itself on that network, so your phone can
reach it from anywhere (home, office, mobile data) while nobody else on the
internet can. No port is opened on your router.

Phones also require a secure (`https://`) address before they will install a
web app or allow notifications. Tailscale provides that too, with a real
certificate for a name like `my-laptop.tail1234.ts.net`.

## What the guide does, step by step

### 1. It looks at Tailscale

It checks whether Tailscale is installed, running, signed in, and whether your
tailnet has the two features Aether needs: **MagicDNS** (a name for each
device) and **HTTPS certificates**. If something is missing it says what, shows
the exact instructions for your computer, waits for you, and checks again. It
never runs `sudo` for you and never downloads and runs a script; where a command
needs administrator rights it prints the command and waits while you run it.

| Your computer | What it tells you to do |
| --- | --- |
| Debian, Ubuntu, Fedora and most Linux | Install with Tailscale's own installer (`curl -fsSL https://tailscale.com/install.sh \| sh`, which you can read first), then `sudo tailscale up` |
| Arch | `sudo pacman -S tailscale`, then `sudo systemctl enable --now tailscaled` |
| Alpine | `sudo apk add tailscale`, then start the service |
| NixOS | `services.tailscale.enable = true;` in your configuration |
| macOS | Install Tailscale from the Mac App Store (or `brew install --cask tailscale`) and open it once |
| Windows with WSL | Install Tailscale **for Windows**, sign in there, and run the guide again from WSL. It uses the Windows Tailscale, so nothing is installed inside WSL. WSL's default `localhost` forwarding must be on |

**Turning on MagicDNS and HTTPS.** Open
<https://login.tailscale.com/admin/dns>, make sure **MagicDNS** is on, scroll to
**HTTPS Certificates** and choose **Enable HTTPS**. This is a one-time change
for your whole tailnet. Tailscale records the names of your machines (such as
`my-laptop.tail1234.ts.net`) in a public certificate log, as it does for every
HTTPS certificate. Your traffic and your data are not in it, only the names.
Pick a computer name you are content to have listed.

If your tailnet is managed by an administrator at work, they may need to do
this step, or approve your computer at
<https://login.tailscale.com/admin/machines>.

### 2. It turns on Aether's remote listener

It starts the Aether daemon if it is not running, then enables remote access
through the same operation as `aether remote enable`. The listener only opens
on `127.0.0.1`, which means this computer itself. Nothing is reachable from the
network yet. It also asks whether to send notifications to your phone;
notifications say that an agent needs you but never include what the agent
wrote.

### 3. It publishes Aether on your tailnet

It runs `tailscale serve --bg --https=443 http://127.0.0.1:8377`, which makes
Tailscale present the listener at `https://<your-computer>.<your-tailnet>.ts.net`
and keeps doing so in the background. The address works only for devices on
your tailnet. It then checks that the address answers and that Aether accepts
it, so a mistake shows up now, not on your phone.

If you already publish something else on port 443, it chooses 8443 or 10000
and tells you the address it ended up with. It never replaces something you
set up.

On Linux, Tailscale may refuse the command until your user is allowed to manage
it. The guide prints `sudo tailscale set --operator=$USER` and waits for you to
run it.

### 4. It pairs your phone

1. **Install the Tailscale app** on your phone: [iPhone and iPad](https://apps.apple.com/app/tailscale/id1470499037),
   [Android](https://play.google.com/store/apps/details?id=com.tailscale.ipn).
   The guide shows a QR code for each store.
2. **Sign in with the same account** you used on the computer. The Tailscale app
   must show it as connected.
3. **Scan the pairing QR code** the guide prints with the phone's camera and
   open the link. If it will not scan, open the address in the guide and type
   the short code.

   **On an iPhone or iPad, add Aether to the Home Screen before you pair.** The
   page that opens shows how: tap Share, then **Add to Home Screen**, open
   Aether from the new icon, and type the code there. iOS gives a Home Screen
   app its own storage, so a pairing made in Safari stays in Safari and the app
   would ask you to pair again.
4. **Compare the three words.** Your phone and your terminal both show three
   words, for example `amber-otter-canyon`, along with the phone's name. If they
   match and it is your phone, answer yes. If they do not match, or you did not
   just scan a code, answer no: someone else may have used it first.

The code works once and expires after two minutes. If it runs out, the guide
offers a new one.

Last, on the phone:

- **iPhone or iPad:** in the Home Screen app, turn notifications on in
  Aether's settings and tap **Allow**. iPhones deliver notifications only to
  apps on the Home Screen. If you paired in Safari instead, tap Share, then
  **Add to Home Screen**, open Aether from the new icon, and pair the app too
  with `aether remote pair`.
- **Android (Chrome):** tap the ⋮ menu, then **Install app** (or **Add to Home
  screen**), open Aether from the new icon, and turn on notifications in
  Aether's settings.

### 5. It shows what it did

A summary of each step, and the commands to manage things afterwards:

| Command | Does |
| --- | --- |
| `aether remote status` | What is on and who is paired |
| `aether remote devices` | List paired phones and computers |
| `aether remote pair` | Add another phone |
| `aether remote revoke NAME` | Cut one device off immediately |
| `aether remote disable` | Close remote access and every connection |
| `tailscale serve reset` | Stop publishing on Tailscale |

To undo everything: `aether remote disable`, then `tailscale serve reset`.

## Running it again

`aether remote setup` is safe to run at any time. If everything is already in
place it changes nothing, reports that it is complete, and offers to pair
another device. After a reinstall, a new computer name, or a Tailscale change,
run it again and it repairs only what moved.

A re-run keeps your settings. In particular it never switches pairing back on:
if you turned it off with `aether remote enable --no-pairing`, it stays off,
the pairing step is skipped, and the guide prints the `aether remote enable`
command that turns it on. Only the first run, when remote access was off,
enables pairing.

## Options

| Flag | Meaning |
| --- | --- |
| `--yes`, `-y` | Accept every default and never wait. Stops at anything only you can do (installing Tailscale, signing in, pairing a device) and prints it. Pairing waits for a person, so it is never started: the run ends with `aether remote pair` as the next step and no device is accepted |
| `--json` | Print one JSON report and nothing else. On its own it only looks and changes nothing; with `--yes` it also makes the changes |
| `--proxy-url https://…` | You already have an `https://` address that reaches Aether (your own reverse proxy, Cloudflare Tunnel, another overlay network). Skips the Tailscale steps and uses that address. See [using your own proxy](#using-your-own-proxy) |
| `--funnel` | Publish to the **public internet** with Tailscale Funnel. See the warning below |

The `--json` report has `status` (`complete`, `pending`, `blocked` or
`stopped`), one entry per step with its `state`, a `detail`, the `command`
that would run or did and any `fix` steps, and a `next` list. A script should
read `status`; the exit code is zero whenever the report was produced. The
pairing code never appears in it.

## Using your own proxy

```sh
aether remote setup --proxy-url https://aether.example.com
```

Point the proxy at `http://127.0.0.1:8377` and have it pass `Host` and
`Origin` through unchanged and support WebSockets. The guide adds the address
to Aether's allowlist, checks that it reaches Aether and pairs your phone.
Everything in the security section below applies to your proxy and not to
Tailscale, so only use one that is private to you.

## Funnel: the public internet

By default only your own devices can reach Aether. `--funnel` changes that:

```sh
aether remote setup --funnel
```

With Funnel, **anyone on the internet** can reach Aether's sign-in page. They
cannot do anything without a paired device, but the page and the pairing
endpoint become public, and anyone can try to guess codes at them. The guide
asks you to type the word `public` before it does this, and with `--yes` it
does so without asking because you asked for it by name. Use it only when your
phone cannot run the Tailscale app. To go back to private, run
`tailscale serve reset` and then `aether remote setup`.

## What protects you

**Tailscale** decides who can reach the address at all: only devices on your
account. Traffic between them is encrypted end to end, and the `https://`
certificate means the phone talks to the real computer.

**Pairing** decides which of those devices can do anything. A phone holds a
key that never leaves it, and Aether remembers the matching public key. There
is no password, token or cookie to steal. Pairing needs you at the computer's
terminal, with a one-time code and the three-word comparison, so a stranger on
your tailnet (a shared device, a coworker) cannot add themselves.

**A lost or stolen phone.** Whoever holds an unlocked paired phone can use
Aether as a device with the role you gave it, so revoke it at once:

```sh
aether remote devices
aether remote revoke "Robin's iPhone"
```

Revoking closes its open connections immediately and it can never connect again.
Also remove the phone from your Tailscale account at
<https://login.tailscale.com/admin/machines>, and lock or erase it with your
phone's own tools.

**What this does not do.** An agent a paired phone approves can run commands
and edit files on your computer, exactly as if you approved it at the keyboard.
Treat the phone like the computer's keyboard. A device paired as an
*observer* can look but not act. Remote access is off until you turn it on and
stops with `aether remote disable`.

## Troubleshooting

**"Tailscale is not installed" after you installed it.** Open a new terminal so
your shell finds the command, then run `aether remote setup` again. On WSL, the
Windows Tailscale must be installed and signed in; the guide looks for
`tailscale.exe`.

**"Tailscale is installed but not running."** Start the service
(`sudo systemctl enable --now tailscaled` on Linux) or open the Tailscale app
(macOS, Windows).

**"HTTPS certificates are off".** Open <https://login.tailscale.com/admin/dns>
and choose **Enable HTTPS**. If you are not the tailnet's administrator, ask
them.

**"Access denied" or "permission" when publishing (Linux).** Run
`sudo tailscale set --operator=$USER` once, then continue.

**"Serve is not enabled on your tailnet."** Tailscale prints a link and then
waits for you to approve the feature. The guide stops waiting after a minute
and shows you that link; open it, approve the feature, then run the guide
again.

**"could not tell which HTTPS ports Tailscale is already using".** The output
of `tailscale serve status --json` has parts this version of Aether does not
understand, so it will not guess that a port is free. Run `tailscale serve
status` to see what is published; if none of it needs to stay, `tailscale serve
reset` clears it.

**The address does not answer.** The certificate is issued the first time the
address is used, which can take a little while. The guide retries for about half
a minute on its own. If it still fails, run `tailscale serve status` and check the address matches the one
the guide printed, and that Tailscale is connected on this computer.

**The phone cannot open the address.** Open the Tailscale app on the phone and
check that it is connected and signed in to the same account as the computer.
Phones on a work profile or with a VPN that blocks other VPNs may need the
Tailscale app switched on explicitly.

**The phone shows "invalid code".** The code was used already, ran out after
two minutes, or was mistyped five times. Run `aether remote pair` for a new one.
If you did not use it yourself, someone else may have; the three words on your
terminal will not match your phone, and you should answer no.

**The three words do not match.** Answer no. Nothing is saved for a device you
reject. Start again with `aether remote pair`.

**The Home Screen app on an iPhone asks to pair again.** That is expected if you
paired in Safari: iOS keeps the app's storage apart from Safari's. Run
`aether remote pair` and type the new code into the app.

**Notifications do not arrive on an iPhone.** They only work for Aether opened
from the home-screen icon, with notifications allowed in Aether's settings and
in iOS Settings > Notifications.

**A different computer name.** If the computer is renamed, the address changes.
Run `aether remote setup` again and pair the phone again from the new address.

**Nothing works and you want to start over.**

```sh
aether remote disable
tailscale serve reset
aether remote setup
```

`aether doctor` has a `remote` line that shows whether the listener is open,
which address it allows and how many devices are paired.
