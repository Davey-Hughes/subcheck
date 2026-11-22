// A read GCC can only call maybe uninitialized, x when read is set and set is not. Nothing links
// this: a test builds it, and passes when GCC refuses it for that read (CMakeLists.txt says why).
int subcheck_canary(bool set, bool read, int value) {
    int x;
    if (set) {
        x = value;
    }
    return read ? x : 0;
}
