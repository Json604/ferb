package com.ferb.app.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import com.ferb.app.FerbViewModel
import com.ferb.app.R
import com.ferb.app.Sheet
import com.ferb.design.FerbIcons
import com.ferb.design.FerbType
import com.ferb.design.components.Card
import com.ferb.design.components.LocalSkyInk
import com.ferb.design.components.PairAction
import com.ferb.design.components.PhoneText
import com.ferb.design.components.Pill
import com.ferb.design.components.PillKind
import com.ferb.design.components.QrFrame
import com.ferb.design.screens.PhoneInsets
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import zxingcpp.BarcodeReader

/**
 * The pairing code's scanner (kb PHN-447): CameraX's preview and analysis with zxing-cpp's reader, QR codes only,
 * normal and light-on-dark; no Google Play services and no ML Kit. The camera is asked for here, in context, and
 * only frames go to the reader: nothing is recorded or kept. The first FERB code read pairs; "Type code" is the way
 * without the camera.
 */
@Composable
fun Scanner(vm: FerbViewModel, insets: PhoneInsets, close: () -> Unit) {
    val context = LocalContext.current
    var allowed by remember { mutableStateOf(context.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    var denied by remember { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> allowed = ok; denied = !ok }
    ScannerPage(
        insets,
        camera = when { allowed -> CameraAccess.ALLOWED; denied -> CameraAccess.DENIED; else -> CameraAccess.NOT_ASKED },
        onAllow = { ask.launch(Manifest.permission.CAMERA) },
        onTypeCode = { vm.show(Sheet.TypeCode) },
        onCancel = close,
    ) { CameraPreview { code -> vm.pair(code) } }
}

/** Where the camera stands for FERB on this page. */
enum class CameraAccess { ALLOWED, NOT_ASKED, DENIED }

/**
 * The scanner page as it is drawn (stateless, for the screenshots): the QR frame holding [preview] (the camera's
 * picture, clipped to the frame's well, kb PHN-618), or why the camera is needed, and under it the lab's white card
 * with the title, Allow the camera while it isn't asked, Type code and Cancel. The iPhone draws the same page
 * (kb PHN-619).
 */
@Composable
fun ScannerPage(
    insets: PhoneInsets,
    camera: CameraAccess,
    onAllow: () -> Unit,
    onTypeCode: () -> Unit,
    onCancel: () -> Unit,
    preview: @Composable () -> Unit,
) {
    val ink = LocalSkyInk.current
    Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally) {
        Spacer(Modifier.weight(1f))
        if (camera == CameraAccess.ALLOWED) {
            QrFrame(Modifier.padding(horizontal = 40.dp), hint = stringResource(R.string.scan_hint), live = true, preview = preview)
        } else {
            PhoneText(stringResource(if (camera == CameraAccess.DENIED) R.string.scan_denied else R.string.scan_camera_why), 16f, FerbType.Medium, ink.top,
                Modifier.padding(horizontal = 32.dp))
        }
        Spacer(Modifier.weight(1f))
        Card(Modifier.padding(start = 16.dp, end = 16.dp, bottom = insets.bottom + 8.dp), padding = 20.dp) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                PhoneText(stringResource(R.string.scan_title), 24f, FerbType.Semibold)
                if (camera == CameraAccess.NOT_ASKED) Pill(stringResource(R.string.scan_allow), icon = FerbIcons.qrScan, onClick = onAllow)
                Row {
                    PairAction(FerbIcons.keyboard, stringResource(R.string.scan_type_code), Modifier.weight(1f), onClick = onTypeCode)
                }
                Pill(stringResource(R.string.cancel), kind = PillKind.PLAIN, height = 44.dp, onClick = onCancel)
            }
        }
    }
}

/** The camera's preview with the reader on its frames; [found] gets the first FERB code once. */
@Composable
private fun CameraPreview(found: (String) -> Unit) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val done = remember { AtomicBoolean(false) }
    val worker = remember { Executors.newSingleThreadExecutor() }
    val reader = remember {
        BarcodeReader().apply {
            options.formats = setOf(BarcodeReader.Format.QR_CODE)
            options.tryInvert = true
            options.tryRotate = true
        }
    }
    DisposableEffect(Unit) { onDispose { worker.shutdown() } }
    AndroidView(
        factory = { c ->
            // TextureView, not SurfaceView (kb PHN-618): a SurfaceView is its own window layer that Compose can't clip
            // or move, so on a phone the picture showed as a rectangle of its own size beside the frame's rounded well.
            // A TextureView draws inside the app's own frame, so the well's clip and every placement apply to it.
            val view = PreviewView(c).apply {
                implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                scaleType = PreviewView.ScaleType.FILL_CENTER
                setBackgroundColor(android.graphics.Color.BLACK)
            }
            val future = ProcessCameraProvider.getInstance(c)
            future.addListener({
                val provider = future.get()
                val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
                val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
                analysis.setAnalyzer(worker) { image ->
                    image.use {
                        if (done.get()) return@use
                        val text = runCatching { reader.read(it) }.getOrNull()?.firstOrNull { r -> r.text?.startsWith("FERB-", ignoreCase = true) == true || r.text?.contains("ferb", ignoreCase = true) == true }?.text
                        if (text != null && done.compareAndSet(false, true)) view.post { found(text) }
                    }
                }
                provider.unbindAll()
                runCatching { provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis) }
            }, context.mainExecutor)
            view
        },
        modifier = Modifier.fillMaxSize().clipToBounds(),
        onRelease = { runCatching { ProcessCameraProvider.getInstance(context).get().unbindAll() } },
    )
}
