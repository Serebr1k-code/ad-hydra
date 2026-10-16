;; ABI lifecycle fixture only: emits no parsed messages; never alters transport bytes.
(module
 (memory (export "memory") 16 64)
 (data (i32.const 0) "{\22messages\22:[],\22state\22:[]}")
 (func (export "hydra_abi_version") (result i32) i32.const 1)
 (func (export "hydra_alloc") (param i32) (result i32) i32.const 1024)
 (func (export "hydra_decode") (param i32 i32) (result i64) i64.const 26)
 (func (export "hydra_dealloc") (param i32 i32 i32 i32))
)
