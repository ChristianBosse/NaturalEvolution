# Looking after a NEVO

For the person who looks after a NEVO node: checking disk space, updating, moving to new hardware, and recovering when storage fails. It assumes the node was set up with [Setting up a NEVO node](setup.md): NEVO runs as the `nevo` service, with its data in `/var/lib/nevo`.

> **Guide version:** first edition, for NEVO 0.1 (October 2026). Parts marked **Coming soon** are planned but not built yet.

## Who can do what today

Accounts don't exist yet (**coming soon**: reader, contributor, curator and admin roles). Until then:

- **Anyone who can open the NEVO page can read and add guides.** Control who joins the Wi-Fi (with a password) if that matters.
- **Nobody can edit or delete a guide from the web page.** Removing content is coming soon. Don't delete files by hand (see "Disk space").
- **Anyone who can touch the node can take its data,** including its secret key. Keep the node somewhere safe.

---

## The data folder

Everything NEVO knows is in one folder: `/var/lib/nevo` (or the folder given with `--data`). The program itself holds no data. With this folder, you can rebuild the node anywhere.

```
/var/lib/nevo/
  node.key                  the node's secret key: its identity
  library/
    objects.sqlite          the library database: every guide, its versions and details
    objects.sqlite-wal      recent changes, while NEVO runs (part of the database)
    objects.sqlite-shm      a helper file, while NEVO runs
    blobs/                  every file: guide texts, photos, PDFs, videos
    tmp/                    uploads in progress
```

| Part | What to know |
|---|---|
| `node.key` | 32 bytes, created at the first start. Every guide added on this node is signed with it, so others can tell it really came from this node. **Keep it safe and secret.** Anyone who has it can pretend to be this node. If it is lost, the node gets a new identity (see below). |
| `objects.sqlite` | The library's index and history. Small compared to the files. The `-wal` and `-shm` files belong to it: always copy all three together, with NEVO stopped. |
| `blobs/` | Files are named after their fingerprint (a hash of their content), in sub-folders such as `blobs/a1/9f/`. The same file uploaded twice is stored once. A damaged file can be detected, because it no longer matches its name. |
| `tmp/` | Uploads are written here first, then moved into `blobs/` once complete. Leftovers from an interrupted upload (a `.part` file) are removed automatically when NEVO starts, once they are an hour old. |

Warnings:

- **Never edit, rename or delete anything inside this folder by hand,** except as described in this guide. Guides point to their files by fingerprint, and their signatures break if anything changes.
- **Never run two nodes with the same `node.key` at the same time.** When sync arrives, other nodes would see one node contradicting itself. To build a second node, start it with an empty data folder: it makes its own key.

---

## Disk space

### Check it

```
df -h /var/lib/nevo
```

Look at the **Avail** (available) and **Use%** columns.

To see how much the library uses:

```
sudo du -sh /var/lib/nevo/library/blobs
```

### Keep space free

NEVO keeps a safety margin of free space (at least 512 MB, or 5% of the disk). When free space drops below it, NEVO refuses new uploads with an "almost out of storage space" page, so the database always has room to save. The home page shows how much space is free. Files left behind by failed uploads are removed automatically after a day.

- Keep at least 10% of the disk, and at least a few GB, free.
- Check after big uploads, and once a month on a busy node.
- One upload can be up to 4 GB. Ask people to keep videos short.

### If space runs low

1. Clear old system logs (safe; they are only messages):

   ```
   journalctl --disk-usage
   sudo journalctl --vacuum-size=100M
   ```

2. Clear downloaded update files: `sudo apt clean`
3. Move the node to bigger storage, such as a larger USB SSD (see "Move a node to new hardware"). This is the real fix.

You can't remove guides yet. **Coming soon:** removing content from this node, and automatic clean-up of files left behind by failed uploads (today these stay in `blobs/` and use some space).

---

## Update the program

Updates need internet, so you need a way online: an Ethernet cable to a router with internet, or the Pi's Wi-Fi on a network with internet (which turns off the hotspot while you update).

**Coming soon:** updates from a Seed stick, with no internet on the node.

You need:

- About an hour, mostly waiting
- The `NaturalEvolution` folder from the first setup (in your home folder)
- A fresh backup (see [setup.md, Part 7](setup.md#part-7-backups-and-the-vault-routine))

Steps:

1. Make a backup. Updates are designed to keep your data, but a backup costs little.
2. Get online. If you use the Wi-Fi: `sudo nmcli connection down nevo-hotspot` (phones lose the connection until step 9).
3. Update the system:

   ```
   sudo apt update && sudo apt full-upgrade -y
   ```

4. Get the new NEVO source code and build it:

   ```
   cd ~/NaturalEvolution
   git pull
   . "$HOME/.cargo/env"
   rustup update
   cargo build --release -p nevo-node
   ```

5. Keep the old program, in case the new one has a problem:

   ```
   sudo cp /usr/local/bin/nevo /usr/local/bin/nevo.old
   ```

6. Stop NEVO, install the new program, start it again:

   ```
   sudo systemctl stop nevo
   sudo install -m 755 target/release/nevo /usr/local/bin/nevo
   sudo systemctl start nevo
   ```

7. Check it runs: `systemctl status nevo` (look for `active (running)`, press **q** to leave).
8. Open NEVO on a phone and check a few guides.
9. Turn the hotspot back on: `sudo nmcli connection up nevo-hotspot`

### If the new version doesn't work

Put the old one back:

```
sudo systemctl stop nevo
sudo cp /usr/local/bin/nevo.old /usr/local/bin/nevo
sudo systemctl start nevo
```

If NEVO still doesn't start, read its messages (`journalctl -u nevo -n 50`) and restore your backup (see "Restore a backup").

---

## Move a node to new hardware

Use this to move to a bigger disk, from an SD card to an SSD, or from a Pi to a PC. The library, its history and the node's identity all come along, because they are all in the data folder. The folder works the same on any computer that runs NEVO.

Warnings first:

- **The backup contains `node.key`.** Keep the USB drive with you and wipe it or store it safely afterwards.
- **Once the new node runs, never start the old one with the same data again.** Two nodes with one key cause trouble when sync arrives. Wipe the old storage, or keep it unplugged as a backup only.
- **Don't let NEVO start on the new hardware before you copy the data in.** A first start creates a new, empty library with a new key. (If that happened, it's fine: the steps below remove it.)

You need:

- The old node, still working
- The new hardware, set up with [setup.md](setup.md) Parts 2, 3 and 5, and Part 4 **only up to step 4** (the service file saved, but not started)
- A USB drive with enough free space

Steps:

1. On the **old** node, make a backup (setup guide, Part 7). Note the name of the backup folder, such as `nevo-backup-2026-10-22`.
2. Shut the old node down: `sudo poweroff`. Keep it off from now on.
3. On the **new** node, plug in the USB drive and restore the backup (see "Restore a backup" below).
4. Open NEVO on a phone and check that your guides are there.

---

## Restore a backup

You need:

- NEVO installed and its service file created (setup guide, Parts 3 and 4)
- The backup on a USB drive, opened at `/media/yourname/BACKUP` (or see "Opening a USB drive by hand")

Steps:

1. Stop NEVO (it's fine if it wasn't running):

   ```
   sudo systemctl stop nevo
   ```

2. Remove any data folder NEVO created on this device. **Make sure you are on the right device:** this deletes its library.

   ```
   sudo rm -r /var/lib/nevo
   ```

   (If it says "No such file or directory", that's fine.)

3. Copy the backup in. Replace the folder name with your backup's name. Note the `/.` at the end of the first path:

   ```
   sudo mkdir /var/lib/nevo
   sudo cp -r /media/yourname/BACKUP/nevo-backup-2026-10-22/. /var/lib/nevo/
   ```

4. Give the folder back to the `nevo` user and make the key private:

   ```
   sudo chown -R nevo:nevo /var/lib/nevo
   sudo chmod 700 /var/lib/nevo
   sudo chmod 600 /var/lib/nevo/node.key
   ```

5. Start NEVO and make it start at every boot:

   ```
   sudo systemctl enable --now nevo
   ```

6. Check it is the same node. Compare the first line of NEVO's messages with the old node's (it starts with `NEVO node`):

   ```
   journalctl -u nevo -n 20
   ```

---

## Recovering from a dead SD card

SD cards wear out. Signs: the Pi doesn't start, the system says "read-only file system", or NEVO's messages show "I/O error".

### If you have a backup

1. Get a new SD card or, better, a USB SSD.
2. Follow [setup.md](setup.md) Parts 2, 3 and 5, and Part 4 up to step 4 (service file saved, not started).
3. Follow "Restore a backup" above.

Anything added after the backup is lost, unless you can save it from the old card (next section).

### If you have no backup (or need newer guides from the old card)

The card may still be readable in another computer.

You need:

- A working Linux computer, such as the Pi with its new card (Windows and Mac can't read the Pi's system partition without extra tools)
- A USB card reader for the old card

Steps:

1. Put the old card in the card reader and plug it into the working Pi. With a desktop, it opens by itself; its system part is called `rootfs`. (On Lite, see "Opening a USB drive by hand"; the system part is usually the second, such as `/dev/sdb2`.)
2. Check the NEVO data is there:

   ```
   sudo ls /media/yourname/rootfs/var/lib/nevo
   ```

   You should see `node.key` and `library`.

3. Copy it to a safe place first, before anything else, while the card still works:

   ```
   sudo cp -r /media/yourname/rootfs/var/lib/nevo /home/yourname/rescued-nevo
   ```

   If some files give errors, let it finish anyway. Note which ones failed.

4. Restore from `/home/yourname/rescued-nevo` with "Restore a backup" above (in step 3, use `/home/yourname/rescued-nevo/.` as the source).
5. Start NEVO and open some guides. If a photo or file was damaged, its link shows "not found"; the rest of the library still works.

If NEVO won't start with the rescued data, read its messages (`journalctl -u nevo -n 50`):

| Message contains | Meaning | What to do |
|---|---|---|
| `node key: ... must hold exactly 32 bytes` | The key file is damaged | Use `node.key` from a backup. If there is none, see "If the key is lost". |
| `library: ...` | The database is damaged | Use an older backup. Keep the damaged folder; it may be repaired by hand later. |
| `can't listen on ...` | Another program uses port 8080, or NEVO already runs | `sudo systemctl status nevo`; stop the other program or change `--listen`. |

**Coming soon:** a check command that verifies every file against its fingerprint and reports what's damaged.

### If the key is lost

If `node.key` can't be recovered but the library can, remove only the broken key and start NEVO:

```
sudo rm /var/lib/nevo/node.key
sudo systemctl start nevo
```

NEVO creates a new key. All guides stay readable: each keeps the signature it was made with. The node simply continues under a new identity, so guides added before show "From another NEVO" instead of "Added on this NEVO". When sync arrives, other nodes will see it as a new node.

---

## Opening a USB drive by hand

Raspberry Pi OS Lite (without a desktop) doesn't open USB drives automatically.

Warning: **pick the right drive.** If the Pi starts from a USB SSD, that SSD also appears in the list. Writing to the wrong one can destroy the system.

1. Plug the drive in and list the drives:

   ```
   lsblk
   ```

   Find your drive by its size (SIZE column). It has a name such as `sda`, and its part has a name such as `sda1`. A part with `/` or `/boot/firmware` under MOUNTPOINTS is the system: **not that one**.

2. Open it at `/mnt/usb` (replace `sda1` with yours):

   ```
   sudo mkdir -p /mnt/usb
   sudo mount /dev/sda1 /mnt/usb
   ```

3. Use `/mnt/usb` wherever the guides say `/media/yourname/BACKUP`.
4. When done, close it before unplugging:

   ```
   sudo umount /mnt/usb
   ```

---

## Monthly checklist

- [ ] NEVO runs: `systemctl status nevo`
- [ ] Free space is above 10%: `df -h /var/lib/nevo`
- [ ] A backup was made this month, and checked
- [ ] No under-voltage warnings: `journalctl -k | grep -i voltage` (no output is good)
- [ ] The vault copies were refreshed in the last 12 months
- [ ] The card with the Wi-Fi name and address is still on the node
