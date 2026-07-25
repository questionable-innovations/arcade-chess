# Shared UART protocol

`ArcadeProtocol` is allocation-free and shared by the AVR and ESP PlatformIO
projects through `lib_extra_dirs`. Frames are COBS encoded, zero delimited, and
protected by CRC-16/CCITT-FALSE (`init=0xffff`, polynomial `0x1021`).

Run the host checks with:

```sh
sh protocol/test/run-host-tests.sh
```

Numeric message layouts used by the bring-up firmware are documented in
[`../docs/uart-api.md`](../docs/uart-api.md).
