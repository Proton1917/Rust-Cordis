#if PACK_NATIVE_FLAG != 7
#error "CFLAGS parameter missing"
#endif
_Static_assert(sizeof(PACK_LABEL) == 10, "quoted CFLAGS parameter changed");

const char *native_build_location(void) {
    return __FILE__;
}
