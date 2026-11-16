# :core's consumer rules keep JNA and the phone library's bindings (found by name). zxing-cpp's reader is called from
# its native library by name too.
-keep class zxingcpp.** { *; }
