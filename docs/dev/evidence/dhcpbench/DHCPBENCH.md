# DHCPBENCH — the LAN side of flight 34's silence (rmbp-ledger B645, R125)

Seat rmbp-0915/local3, branch `exec-rmbp-local-dhcpbench` off `6f13e5e92`. Read 2026-10-13 ~17:35Z (host clock
11:35 MDT, UTC-6) on the bench host `zen` (Fedora 44), from inside the `ubuntu-box` toolbox through
`flatpak-spawn --host`, **no root**. Nothing was changed on the host: every line below is a read.

## Verdict so far

**No DHCP server is on the dongle's segment, and none has been on any flight since 2026-09-15.
The silence comes from the bench, not from UnaOS.** The host's own records prove it, without root:

1. The host side of the cable is `enp198s0f4u1c2` (cdc_ncm, MAC `c8:a3:62:ec:63:b1`). On every rMBP link-up of
   flight 34, NetworkManager auto-activated **`Wired connection 2` = `ipv4.method manual`, `10.0.1.2/24`**. That is a static
   address with no DHCP server. That host is what answered our `txprobe link who_has=10.0.1.2 … reply_ms=5 dst=ours`
   (the probe asks a peer it HEARD, and the host is the only peer on the cable).
2. The connection that serves DHCP is **`pi-share` (`ipv4.method shared`, NM's dnsmasq, 10.42.0.10–254)**. It is pinned
   to `connection.interface-name: eth0`. The host's NIC comes up as `eth0` and the kernel renames it
   `enp198s0f4u1c2` in the same second, so `pi-share` never matches. `pi-share` auto-activated 51 times
   2026-09-05..09-15, when the NIC on that port kept the name `eth0`. It has auto-activated 0 times since. `Wired connection 2`
   auto-activated 47 times 2026-09-29..10-13, including all three flight-34 link-ups.
3. No `dnsmasq` / `dhcpd` / `bootpd` / `systemd-networkd` process is running now. The last `DHCPDISCOVER` dnsmasq
   ever logged was on 2026-09-13, from another client (`4c:bb:47:25:49:c8`). **Our MAC `9c:69:d3:28:6e:f4` never appears
   in any DHCP line of the journal.** It appears once, at 2026-09-15 09:24 MDT, as the kernel registering that same
   dongle as `eth0` on THIS host under `cdc_ncm`. The dongle itself was plugged into the bench host at that time.

So R125's "currently connected" is true of the cable. But the sharing that "worked last" is not what is armed: the
host has carried a static 10.0.1.2 on the segment for every flight since 09-29. Flight 34's missing OFFER is fully
explained without any defect in UnaOS's DISCOVER. **No code change:** no UnaOS-side fault is proven, and none is
needed to explain the data. Whether our 342-byte DISCOVER reaches the wire is still unmeasured. The capture
below reads it, and B632's `[dhcp] tx-size probe` reads it from the UnaOS side.

When these reads were taken the link was down (`NO-CARRIER`, `unavailable`): the rMBP was not linked.

## Commands and outputs (host, no root)

All run as `flatpak-spawn --host <cmd>`.

### `nmcli con show`
```
NAME                UUID                                  TYPE      DEVICE
Wetware Sync Node   4cbcc1f5-9f89-4205-a4d8-222ba5848d3f  wifi      wlp195s0
lo                  17c4953d-22da-492e-b604-517d65416e3a  loopback  lo
pi-share            c9cf5469-fdf2-49a0-89d1-fa2ef67aa80b  ethernet  --
SpectrumSetup-A97E  57e809ed-0278-4f2d-8373-e5e3aa32509c  wifi      --
tuvok               68ed94d4-acc8-497c-9e1e-f712c4b5c349  wifi      --
Wired connection 1  badfc5d8-cddc-3fd4-acc8-81e4c533c55a  ethernet  --
Wired connection 2  43dd2be3-4408-3b05-af14-dd73a8e6b63b  ethernet  --
```

### `nmcli dev status`
```
DEVICE            TYPE      STATE                   CONNECTION
wlp195s0          wifi      connected               Wetware Sync Node
lo                loopback  connected (externally)  lo
p2p-dev-wlp195s0  wifi-p2p  disconnected            --
enp198s0f4u1c2    ethernet  unavailable             --
```

### `nmcli con show pi-share` (the relevant fields)
```
connection.interface-name:              eth0
connection.autoconnect:                 yes
connection.autoconnect-priority:        0
connection.timestamp:                   1789489799        (2026-09-15 16:29:59Z — last active)
ipv4.method:                            shared
ipv4.addresses:                         --                 (NM default: 10.42.0.1/24)
ipv4.shared-dhcp-range:                 --                 (dnsmasq logged 10.42.0.10 -- 10.42.0.254, lease 1h)
ipv6.method:                            ignore
802-3-ethernet.mac-address:             --
```

### `nmcli -f connection.id,connection.interface-name,connection.autoconnect,connection.autoconnect-priority,connection.timestamp,ipv4.method,ipv4.addresses,ipv6.method,802-3-ethernet.mac-address con show "Wired connection 1"` / `"Wired connection 2"`
```
connection.id:                          Wired connection 1
connection.interface-name:              enp198s0f4u1      (no such device — never matches)
connection.autoconnect:                 yes
connection.autoconnect-priority:        -999
connection.timestamp:                   1771015738        (2026-02-13)
ipv4.method:                            manual
ipv4.addresses:                         10.0.0.2/24

connection.id:                          Wired connection 2
connection.interface-name:              --                (matches ANY ethernet)
connection.autoconnect:                 yes
connection.autoconnect-priority:        -100
connection.timestamp:                   1791905844        (2026-10-13 15:37:24Z — flight 34's last link-down)
ipv4.method:                            manual
ipv4.addresses:                         10.0.1.2/24
ipv6.method:                            auto
```

### `ip addr` (wired + wifi; lo omitted)
```
2: wlp195s0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 … state UP
    link/ether 6a:9c:77:34:16:21 brd ff:ff:ff:ff:ff:ff permaddr 50:2e:91:3f:2f:f4
    inet 192.168.1.210/24 brd 192.168.1.255 scope global dynamic noprefixroute wlp195s0
4: enp198s0f4u1c2: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc fq_codel state DOWN
    link/ether c8:a3:62:ec:63:b1 brd ff:ff:ff:ff:ff:ff
    altname enxc8a362ec63b1
```

### `ip route`
```
default via 192.168.1.1 dev wlp195s0 proto dhcp src 192.168.1.210 metric 600
192.168.1.0/24 dev wlp195s0 proto kernel scope link src 192.168.1.210 metric 600
```

### `ps -eo pid,user,args | grep -E "dnsmasq|bootpd|dhcpd|systemd-networkd" | grep -v grep`
```
(no output, rc=1)      — no DHCP server process on the host
```

### `readlink -f /sys/class/net/enp198s0f4u1c2/device/driver` ; `ethtool enp198s0f4u1c2 | tail -1`
```
/sys/bus/usb/drivers/cdc_ncm
	Link detected: no
```

### `journalctl -k --since 2026-09-10 | grep -E "c8:a3:62|9c:69:d3|renamed from eth0|enp198s0f4u1"`
```
Oct 12 19:13:53 zen kernel: cdc_ncm 8-1:2.0: MAC-Address: c8:a3:62:ec:63:b1
Oct 12 19:13:53 zen kernel: cdc_ncm 8-1:2.0 eth0: register 'cdc_ncm' at usb-0000:c6:00.4-1, CDC NCM (NO ZLP), c8:a3:62:ec:63:b1
Oct 12 19:13:53 zen kernel: cdc_ncm 8-1:2.0 enp198s0f4u1c2: renamed from eth0
Oct 13 01:06:35 zen kernel: cdc_ncm 8-1:2.0 enp198s0f4u1c2: unregister 'cdc_ncm' usb-0000:c6:00.4-1, CDC NCM (NO ZLP)
Oct 13 08:21:00 zen kernel: cdc_ncm 8-1:2.0: MAC-Address: c8:a3:62:ec:63:b1
Oct 13 08:21:00 zen kernel: cdc_ncm 8-1:2.0 eth0: register 'cdc_ncm' at usb-0000:c6:00.4-1, CDC NCM (NO ZLP), c8:a3:62:ec:63:b1
Oct 13 08:21:00 zen kernel: cdc_ncm 8-1:2.0 enp198s0f4u1c2: renamed from eth0
```
The same question asked of the whole journal (`journalctl --since 2026-09-01 | grep -E "DHCP(DISCOVER|OFFER|REQUEST|ACK)|9c:69:d3" | tail -20`):
```
Sep 13 14:09:47 zen dnsmasq-dhcp[837977]: DHCPDISCOVER(eth0) 4c:bb:47:25:49:c8      (last DHCP line ever; not us)
Sep 13 14:09:47 zen dnsmasq-dhcp[837977]: DHCPOFFER(eth0) 10.42.0.171 4c:bb:47:25:49:c8
Sep 13 14:09:47 zen dnsmasq-dhcp[837977]: DHCPREQUEST(eth0) 10.42.0.171 4c:bb:47:25:49:c8
Sep 13 14:09:47 zen dnsmasq-dhcp[837977]: DHCPACK(eth0) 10.42.0.171 4c:bb:47:25:49:c8
Sep 15 09:24:32 zen kernel: cdc_ncm 4-2.1:2.0: MAC-Address: 9c:69:d3:28:6e:f4
Sep 15 09:24:32 zen kernel: cdc_ncm 4-2.1:2.0 eth0: register 'cdc_ncm' at usb-0000:c6:00.0-2.1, CDC NCM (NO ZLP), 9c:69:d3:28:6e:f4
```
The 20-line tail holds only `4c:bb:47:25:49:c8` DORA cycles on 09-13 plus these two kernel lines. No DHCP line
names `9c:69:d3:28:6e:f4`.

### `journalctl -u NetworkManager --since 2026-09-01 | grep -E "pi-share|dnsmasq|shared" | tail -30` (excerpt)
```
Sep 15 10:29:25 zen NetworkManager[1122]: policy: auto-activating connection 'pi-share' (c9cf5469-…)
Sep 15 10:29:25 zen NetworkManager[1122]: device (eth0): Activation: starting connection 'pi-share' (c9cf5469-…)
Sep 15 10:29:26 zen NetworkManager[1122]: dnsmasq-manager: starting dnsmasq...
Sep 15 10:29:26 zen dnsmasq-dhcp[119508]: DHCP, IP range 10.42.0.10 -- 10.42.0.254, lease time 1h
Sep 15 10:29:59 zen dnsmasq[119508]: exiting on receipt of SIGTERM          (the last dnsmasq on this host)
```

### Auto-activations per connection, 2026-09-01..now (counted from the same unit's `policy: auto-activating connection '<name>'` lines, per day)
```
pi-share            2026-09-05 .. 2026-09-15   51 activations (09-05:9 06:6 07:2 08:3 09:10 11:8 12:1 13:9 15:3; device eth0)
Wired connection 2  2026-09-29 .. 2026-10-13   47 activations (device enp198s0f4u1c2)
```

### `journalctl -b -u NetworkManager | grep enp198s0f4u1c2` — flight 34 (MDT = Z-6), against FLIGHT34.md
```
Oct 13 09:01:24 … carrier: link connected; auto-activating connection 'Wired connection 2'; … activated   (15:01:24Z, boot 1 plug)
Oct 13 09:04:15 … activated -> unavailable (reason 'carrier-changed')                                    (15:04:15Z; FLIGHT34 pull 15:04:05)
Oct 13 09:18:01 … carrier: link connected; auto-activating connection 'Wired connection 2'; … activated   (15:18:01Z; re-plug 15:17:55)
Oct 13 09:21:55 … activated -> unavailable (reason 'carrier-changed')                                    (15:21:55Z; reboot 15:21:59)
Oct 13 09:27:55 … carrier: link connected; auto-activating connection 'Wired connection 2'; … activated   (15:27:55Z, boot 2 plug)
Oct 13 09:37:24 … activated -> unavailable (reason 'carrier-changed')                                    (15:37:24Z)
```
Every flight-34 link edge on the UnaOS side has a matching edge here, and the host was static 10.0.1.2 on each.
The earlier activations on Oct 12 (20:27, 22:23 MDT) are the same profile.

### `firewall-cmd --get-active-zones` ; `firewall-cmd --zone=nm-shared --list-all` (head)
```
FedoraWorkstation (default)
  interfaces: wlp195s0
nm-shared
  target: ACCEPT
  services: dhcp dns ssh
```
Once `pi-share` is up, the interface joins `nm-shared`, which admits DHCP. The firewall does not block DHCP as well.

### Not readable without root
`ls /var/lib/NetworkManager/` and `cat /var/lib/NetworkManager/dnsmasq-*.leases` -> `Permission denied`.
The journal answers the same question, because dnsmasq logs every DISCOVER/OFFER it handles.

## Peter's commands

### 1. Arm the sharing (root; persistent NM config, so Peter's call)
Bind `pi-share` to the NIC by MAC instead of by the kernel's transient name. Then the eth0 -> enp198s0f4u1c2 rename
no longer defeats it. Its priority 0 already beats `Wired connection 2`'s -100:
```
sudo nmcli con mod pi-share connection.interface-name "" 802-3-ethernet.mac-address C8:A3:62:EC:63:B1
```
Optional extra safety (stops the static 10.0.1.2 profile from grabbing any wired NIC):
```
sudo nmcli con mod "Wired connection 2" connection.autoconnect no
```
If he prefers binding by name instead: `sudo nmcli con mod pi-share connection.interface-name enp198s0f4u1c2`.
To bring it up at once while the rMBP is linked: `sudo nmcli con up pi-share ifname enp198s0f4u1c2`.

**Verify (no root), with the rMBP booted and linked:**
- `nmcli dev status` -> `enp198s0f4u1c2  ethernet  connected  pi-share`
- `ip -4 addr show enp198s0f4u1c2` -> `inet 10.42.0.1/24`
- `ps -eo pid,args | grep [d]nsmasq` -> a dnsmasq with `--interface=enp198s0f4u1c2 … --dhcp-range=10.42.0.10,10.42.0.254`
- `journalctl -u NetworkManager --since -10min --no-pager | grep dnsmasq-dhcp` ->
  `DHCPDISCOVER(enp198s0f4u1c2) 9c:69:d3:28:6e:f4` then `DHCPOFFER(enp198s0f4u1c2) 10.42.0.x 9c:69:d3:28:6e:f4`.
  On the UnaOS side: `[dhcp] rx` rows for udp 67, then a lease with `ip=10.42.0.x … gw=10.42.0.1`. The `txprobe link`
  target follows whichever peer it heard (`UD_PROBE_IP`, usbnet.rs:2536), so it will ask for 10.42.0.1, not 10.0.1.2.

### 2. The capture during a boot (root)
```
sudo tcpdump -i enp198s0f4u1c2 -n -e -vv 'port 67 or port 68 or arp'
```
Start it before powering the rMBP. If tcpdump refuses because the interface is not up, start it at the first
`link connected` in `journalctl -fu NetworkManager`. What each result means:

| What tcpdump shows | Meaning | Next |
|---|---|---|
| `9c:69:d3:28:6e:f4 > ff:ff:ff:ff:ff:ff … 0.0.0.0.68 > 255.255.255.255.67: BOOTP/DHCP, Request`, DHCP-Message Discover, then a `10.42.0.1.67 > ….68` Offer to `9c:69:d3:28:6e:f4` (or broadcast) | the LAN works; any remaining missing lease is in UnaOS's OFFER RX/parse | read the `[dhcp] rx` / `rx-filter` rows from the same boot |
| Discover seen, **no reply** | the server: sharing is not armed (step 1 not applied, or `nmcli dev status` shows `Wired connection 2`) | redo step 1 |
| Reply sent to the **wrong MAC** (dst neither `9c:69:d3:28:6e:f4` nor broadcast) | the server | file it with the capture |
| ARP `who-has` from `9c:69:d3:28:6e:f4` seen, **no Discover at all** | our TX of the 342-byte frame, or the AX88179's large-frame TX | B632's `[dhcp] tx-size probe len=342 … -> large-tx lost` places it in the dongle. `-> large-tx ok` with no Discover on the wire places it in our DHCP send path. Also check `[dhcp] tx-audit` and that a `[dhcp] discover n=` row printed |
| Nothing at all from `9c:69:d3:28:6e:f4` | no carrier, or the wrong interface | `ethtool enp198s0f4u1c2` -> `Link detected: yes`; `[usbnet] link up` on the UnaOS side |

Without step 1, today's expected capture is row 2 (Discover seen, no reply). That alone confirms this verdict,
and it also proves that the 342-byte TX reaches the wire.
