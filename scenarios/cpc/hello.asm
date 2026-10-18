; Scenario program for the Amstrad CPC (disk, started with RUN"HELLO): prints HELLO through the firmware's TXT OUTPUT
; (BB5Ah) and returns to BASIC.
        .cpu z80
        .org $4000
start:  ld hl,msg
next:   ld a,(hl)
        or a
        ret z
        call $bb5a
        inc hl
        jr next
msg:    .asciiz "HELLO"
