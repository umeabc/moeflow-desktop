# Keep methods annotated with @JavascriptInterface
# BlobBridge.saveBlob must not be removed in release builds
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}

# Keep WebView-related classes
-keepclassmembers class * extends android.webkit.WebViewClient {
    public *;
}
-keepclassmembers class * extends android.webkit.WebChromeClient {
    public *;
}

# Keep data classes used for JSON serialization
-keep class org.basmc.moeflow.data.** { *; }
