package com.ferb.design

import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Backspace
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.outlined.ArrowCircleDown
import androidx.compose.material.icons.outlined.ChatBubbleOutline
import androidx.compose.material.icons.outlined.Cloud
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.DesktopWindows
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Inventory2
import androidx.compose.material.icons.outlined.Key
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.LaptopMac
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.ShoppingCart
import androidx.compose.material.icons.outlined.Smartphone
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.UploadFile
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.ChatBubble
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Checklist
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Code
import androidx.compose.material.icons.rounded.Fingerprint
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.SignalCellularAlt
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material.icons.rounded.Terminal
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.foundation.Image as FoundationImage
import androidx.compose.ui.unit.dp

/**
 * The lab draws SF Symbols, which Apple licenses for its own platforms only. Android draws the closest Material
 * Symbol (androidx material-icons-extended, Apache 2.0), one per SF name, so a later swap is one line here.
 */
object FerbIcons {
    val bubble = Icons.Outlined.ChatBubbleOutline          // bubble.left
    val bubbleFill = Icons.Rounded.ChatBubble              // bubble.left.fill
    val checklist = Icons.Rounded.Checklist                // checklist
    val folder = Icons.Outlined.Folder                     // folder
    val folderFill = Icons.Rounded.Folder                  // folder.fill
    val laptop = Icons.Outlined.LaptopMac                  // laptopcomputer
    val person = Icons.Rounded.Person                      // person.fill
    val chevronRight = Icons.Rounded.ChevronRight          // chevron.right
    val check = Icons.Rounded.Check                        // checkmark
    val eye = Icons.Outlined.Visibility                    // eye
    val stop = Icons.Rounded.Stop                          // stop.fill
    val plus = Icons.Rounded.Add                           // plus
    val mic = Icons.Outlined.Mic                           // mic
    val arrowUp = Icons.Rounded.ArrowUpward                // arrow.up
    val search = Icons.Rounded.Search                      // magnifyingglass
    val desktop = Icons.Outlined.DesktopWindows            // menubar.dock.rectangle
    val doc = Icons.Outlined.Description                   // doc
    val download = Icons.Outlined.ArrowCircleDown          // arrow.down.circle
    val sendFile = Icons.Outlined.UploadFile               // arrow.up.doc
    val phone = Icons.Outlined.Smartphone                  // iphone
    val cloud = Icons.Outlined.Cloud                       // cloud
    val server = Icons.Outlined.Dns                        // server.rack
    val sliders = Icons.Rounded.Tune                       // slider.horizontal.3
    val stopCircle = Icons.Outlined.StopCircle             // stop.circle
    val fingerprint = Icons.Rounded.Fingerprint            // touchid
    val qrScan = Icons.Rounded.QrCodeScanner               // qrcode.viewfinder
    val keyboard = Icons.Outlined.Keyboard                 // keyboard
    val help = Icons.AutoMirrored.Outlined.HelpOutline     // questionmark.circle
    val terminal = Icons.Rounded.Terminal                  // terminal
    val key = Icons.Outlined.Key                           // key
    val lock = Icons.Outlined.Lock                         // lock
    val lockFill = Icons.Rounded.Lock                      // lock.fill
    val backspace = Icons.AutoMirrored.Outlined.Backspace  // delete.left
    val close = Icons.Rounded.Close                        // xmark
    val bell = Icons.Outlined.Notifications                // bell
    val link = Icons.Rounded.Link                          // link
    val cart = Icons.Outlined.ShoppingCart                 // cart
    val photo = Icons.Outlined.Image                       // photo
    val play = Icons.Rounded.PlayArrow                     // play.fill
    val waveform = Icons.Rounded.GraphicEq                 // waveform
    val code = Icons.Rounded.Code                          // chevron.left.forwardslash.chevron.right
    val archive = Icons.Outlined.Inventory2                // shippingbox
    val cellular = Icons.Rounded.SignalCellularAlt         // cellularbars (previews' status bar)
    val wifi = Icons.Rounded.Wifi                          // wifi (previews' status bar)

    /**
     * An SF Symbol set in a font of [points] is about that tall and a little wider; a Material Symbol's glyph fills
     * about 20 of its 24 units. Drawing the Material one in a box of 1.2 x points gives the same visual size.
     */
    const val SF_TO_MATERIAL = 1.2f
}

/** An icon drawn at the size the lab's `.font(.system(size: points))` gives it. Decorative unless [description]. */
@Composable
fun FerbIcon(icon: ImageVector, points: Float, tint: Color, modifier: Modifier = Modifier, description: String? = null) {
    FoundationImage(
        imageVector = icon,
        contentDescription = description,
        modifier = modifier.size((points * FerbIcons.SF_TO_MATERIAL).dp),
        colorFilter = ColorFilter.tint(tint),
    )
}
