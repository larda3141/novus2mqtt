# Hardware Setup

## Test Environment

### Ventilation Unit

| Parameter | Value |
|---|---|
| Device Type | NOVUS 300 |
| Construction | L-VER-DEF (Vertical Left) |
| Production Date | 14/03/2017 |
| S/N | 741929834 |
| Slave Board HW-Version | 10430 |
| TFT SKU | CU-TFT-NOV300-450RD |
| TFT S/N | 521014140 |

### Firmware Versions (from bus)

| Component | Version String |
|---|---|
| Bus Version | 1.7.1 |
| Master (VMC) | SWZ0024B31A |
| Fan Slave | SWZ0025B27A |
| PTC Defroster | ST00064E26E |
| TFT Control Unit #1 | ETA0035E20A |
| TFT Control Unit #2 | ETA0036E31E |
| TFT Control Unit #3 | not connected |

### Host System

| Component | Details |
|---|---|
| Board | Raspberry Pi 4 Model B (4GB) |
| OS | Raspberry Pi OS (64-bit / aarch64) |
| USB-RS485 Adapter | [DSD TECH SH-U09C USB to RS485 Converter](https://www.amazon.de/dp/B016IG6X7I) (FTDI FT232RL chip) |
| Serial Device | `/dev/ttyUSB0` |

## Wiring

### RS485 Bus Connection

The Paul Novus exposes its internal RS485 bus at the **X1 connector** on the master controller PCB (located on the adapter board inside the unit). The bus uses a 5-wire cable (J-Y(ST)Y 2x2x0.6):

| X1 Pin | Wire Color | Function |
|---|---|---|
| 1 | Red | 24V Power (do NOT connect to USB adapter) |
| 2 | White | Data (RX/TX+) |
| 3 | Yellow | Data (RX/TX-) |
| 4 | Black | GND |
| 5 | Silver | Shield |

### Connecting the USB-RS485 Adapter

Connect **3 wires** from the X1 bus connector to the USB-RS485 adapter:

```
Paul Novus X1          USB-RS485 Adapter
─────────────          ─────────────────
Pin 2 (White)  ──────  A (Data+)
Pin 3 (Yellow) ──────  B (Data-)
Pin 4 (Black)  ──────  GND
```

**Do NOT connect Pin 1 (24V)** — the USB adapter is powered by USB.

### Where to Find the X1 Connector

1. Open the maintenance flap on the front of the Novus unit
2. The adapter board is located on the left side (when facing the unit)
3. X1 is a 5-pin screw terminal block on the adapter board
4. The existing TFT touch panel cable connects to X1 via a CAT5 cable to X3/X4 bus connectors
5. To tap into the bus, connect your USB adapter in **parallel** to the existing bus wiring — do not disconnect the TFT panel

Alternatively, you can splice into the CAT5 cable between the adapter board and the TFT panel at any accessible point.

### Serial Port Verification

After connecting the USB adapter:

```bash
# Check the device is recognized
ls -la /dev/ttyUSB*

# Verify the adapter chip
dmesg | grep -i ftdi

# Quick bus sniff (should show data if the Novus is powered on)
stty -F /dev/ttyUSB0 9600 cs8 cstopb -parenb
cat /dev/ttyUSB0 | xxd | head -20
```

### Serial Port Permissions

Add your user to the `dialout` group for serial port access:

```bash
sudo usermod -aG dialout $USER
# Log out and back in for the group change to take effect
```

## Notes

- The USB-RS485 adapter with FTDI FT232RL chip is recommended because it reliably supports the required Mark parity configuration via Linux termios
- The adapter is a passive bus listener — it does not interfere with normal bus operation between the master controller and TFT panels
- Multiple TFT panels (up to 3) can be connected to the bus simultaneously; the USB adapter is an additional passive participant
- If the Novus unit is powered off, no data will appear on the bus
