; for UNIX OS Only

; define global functions
.global set_context
.global switch_context

; first argument of set_context(): x0 is pointer to Register structure 
set_context:
    stp d8, d9, [x0]            ; store(packed) d8 -> [x0], d9 -> [x0 + 8]
    stp d10, d11, [x0, #16]
    stp d12, d13, [x0, #32]
    stp d14, d15, [x0, #48]
    stp x19, x20, [x0, #64]
    stp x21, x22, [x0, #80]
    stp x23, x24, [x0, #96]
    stp x25, x26, [x0, #112]
    stp x27, x28, [x0, #128]

    ; save stack pointer and link register
    mov x1, sp 
    stp x30, x1, [x0, #144]

    ; return 0
    mov x0, 0
    ret 

switch_context:
    ldp d8, d9, [x0]            ; load(packed) d8 -> [x0], d9 -> [x0 + 8]
    ldp d10, d11, [x0, #16]
    ldp d12, d13, [x0, #32]
    ldp d14, d15, [x0, #48]
    ldp x19, x20, [x0, #64]
    ldp x21, x22, [x0, #80]
    ldp x23, x24, [x0, #96]
    ldp x25, x26, [x0, #112]
    ldp x27, x28, [x0, #128]

    ; load stack pointer and link register 
    ldp x30, x2, [x0, #144]
    mov sp, x2

    ; return 1
    mov x0, 1
    ret 