# The phone library's bindings call into the native library through JNA, which finds classes and methods by name
# (JNA's own documented rules), and UniFFI's generated callbacks are looked up the same way.
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class com.ferb.mobile.** { *; }
