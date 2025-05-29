# nes-disasm

Disassemble a NES ROM with the help of a [.cdl file](https://fceux.com/web/help/CodeDataLogger.html).

## Usage

```console
$ nes-disasm rom.nes -c rom.cdl -o output
```

## Warning

For now, it considers the ROM is using the MMC4 mapper. Can still be used for other ROMs, but the labels might be wrong.
