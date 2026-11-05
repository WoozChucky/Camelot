// expect: 20
// The predefined macros that <stddef.h> and <stdint.h> are built from name the
// types clang's do, and their sizes and limits agree:
// size_t, ptrdiff_t and intptr_t are as wide as a pointer, int64_t is 64 bits.
typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ptrdiff_t;
typedef __INTPTR_TYPE__ intptr_t;
typedef __UINTPTR_TYPE__ uintptr_t;
typedef __INT64_TYPE__ int64_t;
typedef __UINT64_TYPE__ uint64_t;
typedef __INT8_TYPE__ int8_t;
typedef __UINT16_TYPE__ uint16_t;
typedef __INT32_TYPE__ int32_t;

int main(void) {
    int a[10];
    int r = 0;
    r += sizeof(size_t) == __SIZEOF_SIZE_T__ && sizeof(size_t) == sizeof(void *);       // 1
    r += sizeof(ptrdiff_t) == __SIZEOF_PTRDIFF_T__ && sizeof(ptrdiff_t) == sizeof(long); // 2
    r += sizeof(intptr_t) == __SIZEOF_POINTER__ && sizeof(uintptr_t) == __SIZEOF_POINTER__; // 3
    r += sizeof(int64_t) == 8 && sizeof(uint64_t) == 8;                                  // 4
    r += sizeof(short) == __SIZEOF_SHORT__ && sizeof(int) == __SIZEOF_INT__;             // 5
    r += sizeof(int8_t) == 1 && sizeof(uint16_t) == 2 && sizeof(int32_t) == 4;           // 6
    r += (size_t)-1 > 0 && (uintptr_t)-1 > 0 && (uint64_t)-1 > 0;                        // 7
    r += (ptrdiff_t)-1 < 0 && (intptr_t)-1 < 0 && (int64_t)-1 < 0 && (int8_t)-1 < 0;     // 8
    r += sizeof(&a[3] - &a[0]) == sizeof(ptrdiff_t) && &a[3] - &a[0] == 3;               // 9
    r += sizeof(sizeof a) == sizeof(size_t);                                             // 10
    r += __CHAR_BIT__ * sizeof(long long) == 64;                                         // 11
    r += __INT_MAX__ == 2147483647 && __INT_MAX__ + 1u == 2147483648u;                   // 12
    r += __LONG_MAX__ == (long)(~0ul >> 1) && sizeof __LONG_MAX__ == sizeof(long);       // 13
    r += __LONG_LONG_MAX__ == (long long)(~0ull >> 1) && sizeof __LONG_LONG_MAX__ == 8;  // 14
    r += __SCHAR_MAX__ == 127 && __SHRT_MAX__ == 32767;                                  // 15
    r += __SIZE_MAX__ == (size_t)-1 && __UINTPTR_MAX__ == (uintptr_t)-1;                 // 16
    r += __PTRDIFF_MAX__ == (ptrdiff_t)(__SIZE_MAX__ >> 1) && __INTPTR_MAX__ == __PTRDIFF_MAX__; // 17
    r += __INT64_MAX__ == __LONG_LONG_MAX__ && __UINT64_MAX__ == (uint64_t)-1;           // 18
    r += __INT32_MAX__ == __INT_MAX__ && __UINT32_MAX__ == (unsigned)-1;                 // 19
    r += sizeof __SIZE_MAX__ == sizeof(size_t) && sizeof __UINT64_MAX__ == 8;            // 20
    return r;
}
