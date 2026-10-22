# Setting up a NEVO node

This guide turns a Raspberry Pi or an old computer into a NEVO node: a small offline library that phones and laptops can open over Wi-Fi. Print it and keep it with the node.

> **Guide version:** first edition, for NEVO 0.1 (October 2026). NEVO is still young. This guide describes what the program does **today**. Parts marked **Coming soon** are planned but not built yet.

## What works today, and what doesn't yet

| Works today | Coming soon |
|---|---|
| Browse topics | Search |
| Read guides | The built-in AI assistant |
| Add guides with text, photos, PDFs and video | Accounts (reader, contributor, admin) |
| Play video and audio in the browser | Sync between nodes |
| Works on old phones, with no app to install | NEVO Seed (the USB stick that installs and fills a node) |
| | The setup wizard, and the `nevo.local` address with a "join" QR code |
| | Ready-made downloads of the program |

Until the setup wizard exists, setup uses a few typed commands. Type them exactly as shown. Each command is one line, even when the paper is too narrow and it wraps.

---

## Part 1. Choose the hardware

### Which tier?

NEVO runs on three sizes of hardware. A bigger tier gives better AI answers. It does not unlock extra features: every node can do everything.

| Tier | Memory (RAM) | Example hardware | AI answers (coming soon) |
|---|---|---|---|
| **Standard** | 4 GB | Raspberry Pi 4 or Pi 5 with 4 GB; an old PC or laptop with 4 GB running Linux | Simpler, shorter |
| **Enhanced** | 8 GB | Raspberry Pi 5 with 8 GB; a PC or laptop with 8 GB | Smarter, longer |
| **Power** | 16 GB or more | Raspberry Pi 5 with 16 GB; a PC or laptop with 16 GB or more | Best, and faster |

Today's program uses little memory, so any tier works now. Choose for the AI that is coming: if you can afford 8 GB, take 8 GB.

Computers with less than about 4 GB of RAM cannot run NEVO. On Windows, plan for about 8 GB, because Windows itself uses more memory than Linux.

### Shopping list: Raspberry Pi node

You need:

- A Raspberry Pi 4 or Pi 5 with 4 GB of RAM or more
- The official power supply for that Pi (see "Power" below)
- Storage: a good microSD card of 64 GB or more, **or better**, a USB SSD of 128 GB or more (see "Storage" below)
- A case. For a Pi 5, a case with a fan or the official Active Cooler
- For setup only: a computer with a microSD card reader and internet, to prepare the card
- For setup only: one of these, to type commands on the Pi:
  - an Ethernet cable to your router, and that computer on the same network, or
  - a keyboard and a screen (Pi 4 and Pi 5 use micro-HDMI, so you need a micro-HDMI cable or adapter)

The Pi's built-in Wi-Fi is enough for phones to connect. You don't need a separate router.

### Shopping list: old PC or laptop node

You need:

- A PC or laptop with 4 GB of RAM or more and a 64-bit processor
- Built-in Wi-Fi (for the hotspot), or a separate Wi-Fi router
- An SSD of 128 GB or more is best; an old spinning hard drive works but is slower
- A USB stick of 8 GB or more, to install Linux
- Internet access during setup

A laptop is a good choice: its battery already works like a small UPS (see below).

### Power

Warnings first:

- **Use a power supply that gives a steady 5 volts at enough current.** A weak or cheap supply causes random crashes and damaged SD cards. A Pi shows a lightning bolt icon or logs "under-voltage" when power is too weak.
- **Don't pull the plug to turn the node off.** Shut it down first (see "Turning the node off" in Part 4).

What each Pi needs:

| Pi | Power supply |
|---|---|
| Pi 4 | 5.1 V, 3 A, USB-C (the official 15 W supply) |
| Pi 5 | 5.1 V, 5 A, USB-C (the official 27 W supply) is best. A steady 5 V, 3 A supply also runs it, but then the Pi limits its USB ports to 600 mA in total, which may not be enough for a USB SSD. With a 3 A supply, power the SSD from a powered USB hub. |

A Pi node uses about 3 to 8 watts most of the time, and up to about 12 watts for a Pi 5 under heavy work (such as the coming AI).

**Battery backup.** A battery between the supply and the Pi keeps the node running through short power cuts:

- A **UPS HAT** is a small board with batteries that sits on top of the Pi. Pick one made for your Pi model (Pi 4 or Pi 5) that can deliver 3 A or more at 5 V.
- A **power bank** also works if it supports "pass-through charging" (charging and powering the Pi at the same time) without a short cut-out when the mains comes or goes. Many cheap power banks do cut out for a moment, which reboots the Pi. Test yours: unplug its charger while the Pi runs. The Pi must keep running.

**Solar power.** A typical setup is:

1. A solar panel
2. A charge controller
3. A battery (12 V, LiFePO4 is the longest-lasting kind)
4. A 12 V to 5 V USB-C converter rated for at least 3 A (5 A for a Pi 5), then the Pi

As a rough starting point, a Pi node uses about 100 to 200 watt-hours per day. A 100 W panel with a 12 V, 50 Ah battery is a comfortable start in most sunny places, with a few cloudy days of reserve. Ask a local solar installer to size it for your climate.

**Why power cuts are survivable, but best avoided.** NEVO writes every guide in a careful order: files are saved fully before they are added to the library, and the library database waits until each change is safely on disk. If the power fails, you lose at most the guide that was being added at that moment. The node starts again normally. But SD cards and USB drives themselves can be damaged by sudden power loss, especially cheap ones. So use a battery backup when you can, and shut down properly.

**Coming soon:** a power governor that reads the battery level, turns heavy work off when the battery is low, and shuts the node down cleanly before the battery is empty.

### Storage

- **A USB SSD lasts longer than an SD card.** SD cards wear out after many writes and are easily damaged by power cuts. An SSD is faster, holds more, and survives much longer. Both Pi 4 and Pi 5 can start directly from a USB SSD. (A Pi 5 can also use an NVMe SSD on an add-on board.)
- **If you use an SD card,** buy a known brand, marked "A2" or "Endurance", of 64 GB or more. Avoid unbranded cards: many are fake and hold less than the label says.
- **Size.** The program itself is small (about 6 MB). Space goes to the library: guides, photos and videos people add today, and later the big knowledge packs (a full Wikipedia with pictures is over 100 GB). 128 GB is a good start; 256 GB or more if you plan to hold large packs.

---

## Part 2. Install the operating system

These steps are for a Raspberry Pi. For an old PC, install Debian 12 or newer (or Ubuntu) from a USB stick, then go to Part 3. Use a version with NetworkManager if you want the PC to be the Wi-Fi hotspot (Ubuntu Desktop and Debian with a desktop have it).

Warning: writing the operating system **erases everything** on the card or SSD.

You need:

- The microSD card or USB SSD
- A computer with internet and a card reader (or a USB port for the SSD)

Steps:

1. On the computer, download and install **Raspberry Pi Imager** from raspberrypi.com/software.
2. Open Imager. Choose your Pi model.
3. For the operating system, choose **Raspberry Pi OS (64-bit)**. "Lite" (no desktop) is fine if you will use another computer to type commands.
4. Choose your card or SSD as the storage.
5. When Imager offers to customise the settings, say yes and set:
   - **Hostname:** `nevo`. Later, devices that support it can open the node as `nevo.local`.
   - **Username and password:** choose your own. Write them on the last page of this guide.
   - **Wireless LAN country:** your country. **The Wi-Fi hotspot will not work without it.**
   - **Time zone** and keyboard layout.
   - **Wi-Fi network:** you may enter your home Wi-Fi here to get internet during setup. Part 5 turns the Wi-Fi into a hotspot later.
   - **Services:** turn on **SSH** with password login, if you will type commands from another computer.
6. Write the card. When it's done, put it in the Pi (or plug in the SSD), connect Ethernet if you have it, and plug in the power.
7. Wait about two minutes for the first start.

This guide was checked against Raspberry Pi OS based on Debian 12 "Bookworm". Newer versions (Debian 13 "Trixie") use the same commands.

### Open a command window on the Pi

- **With a keyboard and screen:** log in with your username and password. If you see a desktop, open the Terminal.
- **From another computer:** open a terminal (on Windows: PowerShell; on a Mac: Terminal) and type:

  ```
  ssh yourname@nevo.local
  ```

  Replace `yourname` with the username you chose. Type `yes` if asked about a fingerprint, then your password. If `nevo.local` isn't found, find the Pi's address in your router's list of devices and use it instead: `ssh yourname@192.168.1.23`.

Commands starting with `sudo` ask for your password. Nothing shows while you type it; that's normal.

---

## Part 3. Install and run NEVO

**Coming soon:** ready-made downloads, and a Seed stick that installs everything. Until then, you build the program once from its source code, while the Pi still has internet. This takes about 15 to 45 minutes on a Pi.

You need:

- The Pi running, with internet (Ethernet or home Wi-Fi)
- A command window on the Pi

### Build the program

1. Update the system and install the build tools:

   ```
   sudo apt update && sudo apt full-upgrade -y
   sudo apt install -y build-essential git curl
   ```

2. Install Rust (the language NEVO is written in). The version in Raspberry Pi OS is too old, so use the official installer:

   ```
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   . "$HOME/.cargo/env"
   ```

3. Download NEVO's source code and build it:

   ```
   git clone https://github.com/ChristianBosse/NaturalEvolution.git
   cd NaturalEvolution
   cargo build --release -p nevo-node
   ```

   Wait until it finishes. The last line should start with `Finished`.

4. Copy the program to where the system keeps programs:

   ```
   sudo install -m 755 target/release/nevo /usr/local/bin/nevo
   ```

5. Keep the `NaturalEvolution` folder. You need it to update NEVO later.

### Try it once

1. Start NEVO:

   ```
   nevo --data ~/nevo-try
   ```

   It prints three lines:

   ```
   NEVO node n5h3...
   data:   /home/yourname/nevo-try
   open:   http://0.0.0.0:8080/
   ```

   The first line is the node's identity (its public key). `0.0.0.0` means "every network this computer is on". It is not an address you can open.

2. Find the node's address. Open a **second** command window and type:

   ```
   hostname -I
   ```

   It prints one or more numbers such as `192.168.1.23`. The first one is usually right.

3. On a phone or laptop on the same network, open a web browser and go to `http://192.168.1.23:8080` (with your own number). Don't forget `:8080` at the end. You should see the NEVO library page.

4. Go back to the first window and press **Ctrl+C** to stop NEVO.

5. This was a test folder. Delete it, so the real node starts empty:

   ```
   rm -r ~/nevo-try
   ```

### The `nevo` command

```
nevo [--data <folder>] [--listen <address:port>]
```

| Option | What it does | If you leave it out |
|---|---|---|
| `--data <folder>` | Where the library and the node's key are kept | A folder named `nevo-data` in the current folder |
| `--listen <address:port>` | Which network address and port to answer on | `0.0.0.0:8080`: every network, port 8080 |

Always give `--data` with a full path, so NEVO uses the same folder every time. Part 4 sets this up for you.

---

## Part 4. Start NEVO automatically

This makes NEVO start every time the Pi starts, and restart if it ever crashes. It runs NEVO as its own user, named `nevo`, with its data in `/var/lib/nevo`.

You need:

- NEVO installed at `/usr/local/bin/nevo` (Part 3)
- A command window on the Pi

Steps:

1. Create the `nevo` user. It can't log in; it only runs the program.

   ```
   sudo useradd --system --home-dir /var/lib/nevo --shell /usr/sbin/nologin nevo
   ```

2. Create the service file:

   ```
   sudo nano /etc/systemd/system/nevo.service
   ```

3. Type (or paste) exactly this:

   ```ini
   [Unit]
   Description=NEVO offline library
   After=network.target

   [Service]
   Type=simple
   User=nevo
   Group=nevo
   ExecStart=/usr/local/bin/nevo --data /var/lib/nevo --listen 0.0.0.0:8080
   # systemd creates /var/lib/nevo, owned by the nevo user, readable by nobody else.
   StateDirectory=nevo
   StateDirectoryMode=0700
   # NEVO finishes its work and stops cleanly on Ctrl+C (SIGINT).
   KillSignal=SIGINT
   TimeoutStopSec=30
   Restart=on-failure
   RestartSec=5
   # Safety: NEVO can write only to its own data folder.
   NoNewPrivileges=yes
   ProtectSystem=strict
   ProtectHome=yes
   PrivateTmp=yes

   [Install]
   WantedBy=multi-user.target
   ```

4. Save and close: press **Ctrl+O**, then **Enter**, then **Ctrl+X**.

5. Turn the service on and start it now:

   ```
   sudo systemctl daemon-reload
   sudo systemctl enable --now nevo
   ```

6. Check that it runs:

   ```
   systemctl status nevo
   ```

   Look for `active (running)` in green. Press **q** to leave.

### Everyday service commands

| To | Type |
|---|---|
| See if NEVO runs | `systemctl status nevo` |
| Stop NEVO | `sudo systemctl stop nevo` |
| Start NEVO | `sudo systemctl start nevo` |
| Restart NEVO | `sudo systemctl restart nevo` |
| Read NEVO's messages (newest last) | `journalctl -u nevo -n 50` |
| Check that NEVO answers | `curl http://localhost:8080/health` (prints `ok`) |

### Turning the node off

Warning: never pull the plug while the node is running, unless there is no other way.

```
sudo poweroff
```

Wait until the green light on the Pi stops blinking (about 20 seconds), then unplug it.

### Optional: a shorter address without `:8080`

Web browsers use port 80 when no port is given. To let people type `http://10.42.0.1` instead of `http://10.42.0.1:8080`:

1. Open the service file again: `sudo nano /etc/systemd/system/nevo.service`
2. Change the `ExecStart` line to end with `--listen 0.0.0.0:80`.
3. Add this line under `[Service]` (it lets NEVO use port 80 without running as the all-powerful root user):

   ```ini
   AmbientCapabilities=CAP_NET_BIND_SERVICE
   ```

4. Save, then type: `sudo systemctl daemon-reload && sudo systemctl restart nevo`

The rest of this guide assumes port 8080. If you made this change, leave out `:8080` wherever it appears.

---

## Part 5. Let phones connect over Wi-Fi

The Pi can create its own Wi-Fi network (a "hotspot"). Phones join it and open NEVO. No router and no internet are needed.

Warnings first:

- **The Pi's Wi-Fi can't be a hotspot and join another Wi-Fi network at the same time.** When the hotspot starts, the Pi leaves your home Wi-Fi. If you are typing commands over that Wi-Fi, your connection will drop. Do this part with an Ethernet cable or a keyboard and screen.
- **Finish Part 3 first.** After this part the Pi has no internet over Wi-Fi.
- **The Wireless LAN country must be set** (Part 2, step 5). If you skipped it, run `sudo raspi-config`, choose **Localisation Options**, then **WLAN Country**, and pick your country.

These commands are for Raspberry Pi OS Bookworm or newer, which manages networks with NetworkManager (the `nmcli` command). They also work on other Linux systems that use NetworkManager; replace `wlan0` with your Wi-Fi device's name (find it with `nmcli device`).

You need:

- A name for the network, such as `NEVO-Library`
- A password of 8 to 63 characters, if you want one (see the choice below)
- A command window on the Pi, not over Wi-Fi

### Choose: password or open?

- **With a password** (best for a home or a small group): only people who know the password can join. Write the password on a card next to the node.
- **Open, no password** (best for a public library or a school): anyone in range can join with one tap. Remember that, until accounts exist (**coming soon**), **anyone who can open NEVO can add guides**.

### Create the hotspot

1. Create the hotspot settings. Replace `NEVO-Library` with your network name:

   ```
   sudo nmcli connection add type wifi ifname wlan0 con-name nevo-hotspot ssid NEVO-Library
   ```

2. Make it a hotspot on 2.4 GHz (the band that reaches farthest and that old phones support), and let it hand out addresses to phones:

   ```
   sudo nmcli connection modify nevo-hotspot 802-11-wireless.mode ap 802-11-wireless.band bg ipv4.method shared
   ```

3. **Only if you want a password**, type this. Replace `your-password` with your own:

   ```
   sudo nmcli connection modify nevo-hotspot wifi-sec.key-mgmt wpa-psk wifi-sec.psk "your-password"
   ```

4. Make the hotspot start by itself at every boot, ahead of any other Wi-Fi network the Pi knows:

   ```
   sudo nmcli connection modify nevo-hotspot connection.autoconnect yes connection.autoconnect-priority 100
   ```

5. Start it now:

   ```
   sudo nmcli connection up nevo-hotspot
   ```

6. Find the node's address on the hotspot:

   ```
   nmcli -g IP4.ADDRESS device show wlan0
   ```

   It prints something like `10.42.0.1/24`. The address is the part before the `/`: here, `10.42.0.1`.

### Connect a phone

1. On the phone, open the Wi-Fi settings and join the network you named (for example `NEVO-Library`).
2. The phone may warn that this network has **no internet**. Choose to **stay connected** (the wording differs: "Keep", "Use without internet", "Stay connected"). On some phones you also have to turn off mobile data, or the phone keeps using it instead.
3. Open the web browser and type the address with the port: `http://10.42.0.1:8080`
4. Bookmark the page, or add it to the home screen.

Tip: write the network name, password and address on a card and stick it on the node. Many people will ask.

**Coming soon:** the setup wizard will show a QR code to join, and phones will be sent to NEVO automatically when they join (a "captive portal"). The `nevo.local` name may already work on some laptops and newer phones, at `http://nevo.local:8080`, if you set the hostname to `nevo` in Part 2. The number always works.

### If phones can't connect

| Problem | Try |
|---|---|
| The network doesn't appear | Check the country is set (`sudo raspi-config`, Localisation Options, WLAN Country). Then `sudo nmcli connection up nevo-hotspot` and read the error. |
| The phone sees it but fails to join | Some phones fail with newer security settings on the Pi's Wi-Fi chip. Type: `sudo nmcli connection modify nevo-hotspot wifi-sec.proto rsn wifi-sec.pairwise ccmp wifi-sec.group ccmp wifi-sec.pmf disable` then `sudo nmcli connection up nevo-hotspot` |
| The phone joins, but the page doesn't load | Check the address ends with `:8080` and starts with `http://` (not `https://`). Check NEVO runs: `systemctl status nevo`. Turn off mobile data on the phone. |
| You forgot the password | `sudo nmcli -s -g 802-11-wireless-security.psk connection show nevo-hotspot` |

### Turning the hotspot off

To get internet on the Pi again (for an update, for example):

```
sudo nmcli connection down nevo-hotspot
```

The Pi then joins any other Wi-Fi network it knows. The hotspot comes back at the next start, or now with `sudo nmcli connection up nevo-hotspot`.

To remove the hotspot for good: `sudo nmcli connection delete nevo-hotspot`

---

## Part 6. Set the clock

A Raspberry Pi has no clock battery, so offline it may not know the date. NEVO records the date on each guide, and shows it in the guide's history. NEVO doesn't depend on the clock being right, but correct dates help people.

- While the Pi has internet, it sets its clock by itself.
- Offline, set it by hand after each start if it's wrong: `sudo date -s "2026-10-22 14:30"`
- A **Pi 5** has a built-in clock that keeps time when powered off if you add the small official RTC battery to its "BAT" connector.

**Coming soon:** support for a GPS module that gives the true time anywhere.

---

## Part 7. Backups and the vault routine

A node holds knowledge that people may not be able to find again. Back it up.

Warnings first:

- **A backup includes the node's secret key** (`node.key`). Anyone with a backup can pretend to be this node. Store backups as carefully as the node itself.
- **Stop NEVO while you copy.** The library database keeps recent changes in an extra file next to it (`objects.sqlite-wal`). A copy made while NEVO runs can miss changes or be broken. Stopping takes a second; phones just can't load pages during the copy.
- **Flash storage fades when unplugged.** USB sticks, SD cards and SSDs left in a drawer slowly lose data over the years. Refresh long-term copies about once a year (see "The vault routine").

### Make a backup

You need:

- A USB drive with more free space than NEVO's data (check with `sudo du -sh /var/lib/nevo`)
- A command window on the node

Steps:

1. Plug in the USB drive. On Raspberry Pi OS with a desktop, it opens under `/media/yourname/`. Find its exact folder:

   ```
   ls /media/$USER/
   ```

   This guide calls it `/media/yourname/BACKUP`. Use your own name.

   (On Raspberry Pi OS Lite, drives are not opened automatically. See "Opening a USB drive by hand" in the admin guide.)

2. Stop NEVO:

   ```
   sudo systemctl stop nevo
   ```

3. Copy the whole data folder, with today's date in the name:

   ```
   sudo cp -r /var/lib/nevo /media/yourname/BACKUP/nevo-backup-$(date +%F)
   ```

4. Check the copy matches. No output means they are the same:

   ```
   sudo diff -r /var/lib/nevo /media/yourname/BACKUP/nevo-backup-$(date +%F)
   ```

5. Start NEVO again:

   ```
   sudo systemctl start nevo
   ```

6. Safely remove the drive: `sudo umount /media/yourname/BACKUP`, then unplug it.

How often: after any big addition, and at least once a month on a busy node. Keep the two or three most recent backups.

To restore a backup, see "Recovering from a dead SD card" in [Looking after a NEVO](admin.md).

### The vault routine

A "vault" is a long-term copy kept somewhere safe, for the day the node is lost, stolen, flooded or burnt.

1. **Keep two copies, on two kinds of storage.** For example, one on a USB SSD and one on a USB stick or SD card. Different kinds don't tend to fail the same way at the same time.
2. **Keep them in two places.** Not both next to the node. A neighbour's house, a school, a sealed box.
3. **Keep a printed copy of this guide with each vault copy.** Whoever finds the copy must be able to use it without the internet.
4. **Label each copy:** "NEVO backup", the date, and "contains a secret key, keep safe".
5. **Once a year, refresh both copies.** Plug each one in and write a fresh backup over it (steps above). Rewriting the data renews it, which keeps unplugged flash from fading. If a drive gives errors, replace it.
6. **Store copies cool and dry.** Heat speeds up fading.

**Coming soon:** NEVO Seed's vault mode will mark vault drives, remind you when a refresh is due, check every file against its fingerprint, and rewrite weak data for you.

---

## What next

- [Using NEVO](using.md): for everyone who reads and adds guides
- [Looking after a NEVO](admin.md): disk space, updates, moving to new hardware, recovering from a dead SD card

---

## Notes for this node

Fill this in with a pencil and keep it with the node. **Don't** write the node's secret key here.

| | |
|---|---|
| Node name / place | |
| Username on the Pi | |
| Password on the Pi | |
| Wi-Fi network name | |
| Wi-Fi password | |
| Address for phones | http:// |
| Where the backups are | |
| Last backup date | |
| Last vault refresh | |
