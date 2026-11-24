package com.ferb.app

import android.app.Application
import android.os.Looper
import androidx.lifecycle.SavedStateHandle
import androidx.test.core.app.ApplicationProvider
import com.ferb.app.model.AccountModel
import com.ferb.app.model.LocalFiles
import com.ferb.app.model.Provider
import com.ferb.app.model.SignedIn
import com.ferb.app.model.Site
import com.ferb.app.model.Tokens
import com.ferb.core.FAKE_ENDPOINT
import com.ferb.mobile.LinkPhase
import com.ferb.mobile.LinkProblem
import java.io.File
import java.net.URI
import java.net.URLDecoder
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestCoroutineScheduler
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** FERB's account in the app (kb PHN-828, PHN-839, PHN-841): the sign-in gate, the phone-first page, pairing's statement, sign-out. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class AccountFlowTest {
    private val phone = FakePhone()
    private val scheduler = TestCoroutineScheduler()
    private lateinit var session: Session
    private val root: File = Files.createTempDirectory("ferb-account-flow").toFile()
    private val http = FakeHttp()
    private var now = 0L
    private val opened = ArrayList<String>()

    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher(scheduler))
        session = Session { 0L }
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
        root.deleteRecursively()
    }

    private fun vm(account: AccountModel): FerbViewModel =
        FerbViewModel(phone, session, Prefs(ApplicationProvider.getApplicationContext()), { true }, null, SavedStateHandle(), LocalFiles(root) { null },
            wait = {}, io = Dispatchers.Main, steady = { 0L }, account = account).also { it.openBrowser = { url -> opened += url } }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    /** Forgetting the Mac clears files on a real background thread: wait for the work, not the clock. */
    private fun eventually(cond: () -> Boolean) {
        repeat(150) { if (cond()) return; Thread.sleep(20); idle() }
    }

    private fun signedIn(store: MemoryAccountStore = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh")))) =
        testAccount(http, store, { now })

    private fun stateOf(url: String) = URI(url).rawQuery.split('&').map { it.split('=', limit = 2) }
        .first { it[0] == "state" }[1].let { URLDecoder.decode(it, "UTF-8") }

    // decision: PHN-839, PHN-8
    @Test
    fun the_first_launch_shows_the_sign_in_page_after_the_lock_and_nothing_else_while_signed_out() {
        // a phone that is even paired and online: signed out, nothing of FERB's shows
        val vm = vm(signedOutAccount(http) { now })
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        session.unlocked(); idle()
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        // pairing can't start from here: no statement, nothing handed to the link
        vm.pair("FERB-INVENTED"); idle()
        assertTrue(phone.calls.none { it.startsWith("pair") })
        assertTrue("no statement was asked for", http.requests.isEmpty())
        // the lock still comes first
        session.lockNow()
        assertEquals(Gate.LOCKED, vm.ui.value.gate)
        session.unlocked(); idle()

        // Continue with Google: the browser opens the site's start; its return with this state signs in
        vm.signIn(Provider.GOOGLE); idle()
        val start = opened.single()
        assertTrue(start.startsWith("${Site.URL}/api/app/start?provider=google&platform=android"))
        http.reply("/api/app/token", 200, FakeHttp.tokens())
        vm.signInReturned("${Site.CALLBACK}?code=invented-code&state=another-state-00000"); idle()
        assertEquals("another sign-in's return is ignored", Gate.SIGN_IN, vm.ui.value.gate)
        assertTrue(http.requests.isEmpty())
        vm.signInReturned("${Site.CALLBACK}?code=invented-code&state=${stateOf(start)}"); idle()
        assertEquals("someone@example.com", vm.ui.value.account.account?.email)
        assertEquals("signed in, no Mac yet: the pairing gate (its phone-first page)", Gate.PAIR, vm.ui.value.gate)
    }

    // decision: PHN-839
    @Test
    fun the_browser_closed_without_an_answer_brings_the_buttons_back() {
        val vm = vm(signedOutAccount(http) { now })
        session.unlocked(); idle()
        vm.signIn(Provider.APPLE); idle()
        assertEquals(com.ferb.app.model.SignInStep.BROWSER, vm.ui.value.account.step)
        vm.browserLeft()
        assertEquals(com.ferb.app.model.SignInStep.IDLE, vm.ui.value.account.step)
    }

    // decision: PHN-841
    @Test
    fun signed_in_with_no_mac_shows_the_phone_first_page_until_pair_now() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        val vm = vm(signedIn())
        session.unlocked(); idle()
        assertEquals(Gate.PAIR, vm.ui.value.gate)
        assertNull(vm.ui.value.pairReason)
        assertFalse(vm.pairNow.value)
        vm.pairNow(true)
        assertTrue(vm.pairNow.value)
        vm.pairNow(false)
        assertFalse(vm.pairNow.value)
    }

    // decision: PHN-828
    @Test
    fun pairing_carries_the_servers_statement_for_this_phones_own_link_id() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        http.reply("/api/app/assertion", 200, """{"assertion":"fpa1.invented-payload.invented-signature","expires_in":300}""")
        val vm = vm(signedIn())
        session.unlocked(); idle()
        vm.pair("FERB-INVENTED"); idle()
        val asked = http.requests.single()
        assertEquals("${Site.URL}/api/app/assertion", asked.url)
        assertEquals("""{"endpoint":"$FAKE_ENDPOINT"}""", asked.body)
        assertEquals("fa1_invented-access", asked.bearer)
        assertEquals(listOf("pair:FERB-INVENTED"), phone.calls.filter { it.startsWith("pair") })
        assertEquals(listOf("fpa1.invented-payload.invented-signature"), phone.pairAccounts)
        assertTrue(vm.ui.value.pairing)

        // the Mac's daemon refused this phone's account: its own words are the pairing's error
        val words = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."
        phone.link.value = FakePhone.status(LinkPhase.STOPPED, paired = false, problem = LinkProblem("account_mismatch", words, null))
        assertFalse(vm.ui.value.pairing)
        assertEquals(Note(R.string.err_mac_said, words), vm.ui.value.pairError)
        for (code in listOf("mac_signed_out", "account_unconfirmed")) {
            assertEquals(Note(R.string.err_mac_said, "the Mac's words"), FerbViewModel.pairNote(code, "the Mac's words"))
        }
    }

    // decision: PHN-828
    @Test
    fun pairing_offline_says_it_needs_the_internet_and_hands_nothing_to_the_link() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        http.offline = true
        val vm = vm(signedIn())
        session.unlocked(); idle()
        vm.pair("FERB-INVENTED"); idle()
        assertEquals(Note(R.string.pair_needs_internet), vm.ui.value.pairError)
        assertFalse(vm.ui.value.pairing)
        assertTrue(phone.calls.none { it.startsWith("pair") })
    }

    // decision: PHN-839
    @Test
    fun sign_out_forgets_the_mac_then_the_account() {
        val store = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh")))
        val vm = vm(signedIn(store))
        session.unlocked(); idle()
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        vm.show(Sheet.Account)
        vm.signOut()
        eventually { "account" !in vm.ui.value.busy }
        assertTrue("the Mac is forgotten (and told, as Forget this Mac)", "forget" in phone.calls)
        assertEquals(listOf("/api/app/signout"), http.paths())
        assertTrue(http.requests.single().body!!.contains("fr1_invented-refresh"))
        assertNull(store.kept)
        assertNull(vm.sheet.value)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
    }

    // decision: PHN-839
    @Test
    fun delete_must_succeed_on_the_server_before_the_mac_and_the_account_go() {
        val store = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh")))
        val vm = vm(signedIn(store))
        session.unlocked(); idle()
        http.reply("/api/app/delete", 503, """{"error":"unavailable"}""")
        vm.deleteAccount(); idle()
        assertEquals(Note(R.string.account_delete_failed), vm.ui.value.accountNote)
        assertTrue(phone.calls.none { it == "forget" })
        assertNotNull(store.kept)
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        http.replies.clear()
        http.reply("/api/app/delete", 200, """{"deleted":true}""")
        vm.deleteAccount()
        eventually { "account" !in vm.ui.value.busy }
        assertTrue("forget" in phone.calls)
        assertNull(store.kept)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
    }

    // decision: PHN-839, PHN-828
    @Test
    fun the_server_ending_the_sign_in_shows_the_sign_in_page_at_the_next_open_and_an_offline_check_does_not() {
        val store = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 30_000L, "fr1_invented-refresh")))
        val vm = vm(signedIn(store))
        http.offline = true
        session.unlocked(); idle()
        assertEquals("offline: still signed in, FERB keeps working", Gate.MAIN, vm.ui.value.gate)
        assertTrue(vm.ui.value.account.checkFailed)
        session.lockNow()
        http.offline = false
        http.reply("/api/app/token", 401, """{"error":"invalid_grant"}""")
        session.unlocked(); idle()
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        // a sign-out: the Mac is told and forgotten here, as Sign out does (the server isn't told: it ended it)
        eventually { "forget" in phone.calls && "account" !in vm.ui.value.busy }
        assertTrue("forget" in phone.calls)
        assertNull(store.kept)
        assertTrue(http.paths().none { it == "/api/app/signout" })
    }

    // decision: PHN-828
    @Test
    fun a_server_that_refuses_even_a_fresh_token_signs_out_and_unpairs() {
        val store = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh")))
        val vm = vm(signedIn(store))
        session.unlocked(); idle()
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        http.reply("/api/app/assertion", 401, """{"error":"invalid_token"}""")
        http.reply("/api/app/token", 200, FakeHttp.tokens(access = "fa1_new-access"))
        vm.pair("FERB-INVENTED")
        eventually { "forget" in phone.calls && "account" !in vm.ui.value.busy && "pair" !in vm.ui.value.busy }
        assertEquals(listOf("/api/app/assertion", "/api/app/token", "/api/app/assertion"), http.paths())
        assertTrue("forget" in phone.calls)
        assertTrue("nothing handed to the link", phone.calls.none { it.startsWith("pair") })
        assertNull(store.kept)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
    }

    // decision: PHN-839
    @Test
    fun the_persons_own_check_offline_never_unpairs() {
        val store = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 30_000L, "fr1_invented-refresh")))
        val vm = vm(signedIn(store))
        http.reply("/api/app/token", 503, """{"error":"unavailable"}""")
        session.unlocked(); idle()
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        assertTrue(phone.calls.none { it == "forget" })
    }

    private fun legacyPairing() {
        // paired before FERB's account existed: a pairing date, no account recorded for it
        Prefs(ApplicationProvider.getApplicationContext()).pairedSince = 1L
        http.reply("/api/app/assertion", 200, """{"assertion":"fpa1.invented-bind.statement","expires_in":300}""")
    }

    private fun prefs() = Prefs(ApplicationProvider.getApplicationContext())

    // decision: PHN-828
    @Test
    fun an_older_pairing_is_bound_to_the_account_after_sign_in_once() {
        legacyPairing()
        val vm = vm(signedOutAccount(http) { now })
        session.unlocked(); idle()
        assertTrue("nothing is bound while signed out", phone.binds.isEmpty())
        vm.signIn(Provider.GOOGLE); idle()
        http.reply("/api/app/token", 200, FakeHttp.tokens())
        vm.signInReturned("${Site.CALLBACK}?code=invented-code&state=${stateOf(opened.single())}")
        eventually { prefs().pairedAccount != null }
        assertEquals(listOf("fpa1.invented-bind.statement"), phone.binds)
        assertTrue(http.requests.any { it.url.endsWith("/api/app/assertion") && it.body == """{"endpoint":"$FAKE_ENDPOINT"}""" })
        assertEquals("acct-0001", prefs().pairedAccount)
        assertTrue("the Mac's 'forget' was never asked", phone.calls.none { it == "forget" })
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        // bound: never asked again
        session.lockNow(); session.unlocked(); idle()
        assertEquals(1, phone.binds.size)
    }

    // decision: PHN-828
    @Test
    fun a_fresh_pairing_records_its_account_and_never_binds() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        val vm = vm(signedIn())
        session.unlocked(); idle()
        phone.link.value = FakePhone.status()
        idle()
        assertEquals("acct-0001", prefs().pairedAccount)
        assertTrue(phone.binds.isEmpty())
        assertEquals(Gate.MAIN, vm.ui.value.gate)
    }

    // decision: PHN-828
    @Test
    fun a_mac_of_another_account_says_so_and_the_phone_is_removed() {
        legacyPairing()
        val words = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."
        phone.bindScript.add(com.ferb.core.Outcome.Failed(com.ferb.mobile.FerbException.Refused(words, "account_mismatch")))
        val vm = vm(signedIn())
        session.unlocked()
        eventually { vm.ui.value.gate == Gate.REMOVED }
        assertEquals(Gate.REMOVED, vm.ui.value.gate)
        assertEquals(Note(R.string.err_mac_said, words), vm.ui.value.removedNote)
        assertNull(prefs().pairedAccount)
        // Pair again: the existing path forgets the Mac and the words go with it
        vm.forget()
        eventually { "forget" in phone.calls && "forget" !in vm.ui.value.busy }
        assertNull(vm.ui.value.removedNote)
    }

    // decision: PHN-828
    @Test
    fun offline_or_a_mac_signed_out_changes_nothing_and_is_tried_again_at_the_next_unlock() {
        legacyPairing()
        http.offline = true
        val vm = vm(signedIn())
        session.unlocked(); idle()
        assertTrue("no statement, no bind", phone.binds.isEmpty())
        // the same unlock never tries again, however the link moves
        http.offline = false
        phone.link.value = FakePhone.status(LinkPhase.WAITING); idle()
        phone.link.value = FakePhone.status(); idle()
        assertTrue(phone.binds.isEmpty())
        // next unlock: the Mac isn't signed in yet: nothing changes
        phone.bindScript.add(com.ferb.core.Outcome.Failed(com.ferb.mobile.FerbException.Refused("FERB on this Mac isn't signed in yet.", "mac_signed_out")))
        session.lockNow(); session.unlocked()
        eventually { phone.binds.size == 1 }
        assertNull(prefs().pairedAccount)
        assertEquals(Gate.MAIN, vm.ui.value.gate)
        assertNull(vm.ui.value.removedNote)
        assertTrue(phone.calls.none { it == "forget" })
        // and the one after that binds
        session.lockNow(); session.unlocked()
        eventually { prefs().pairedAccount != null }
        assertEquals(2, phone.binds.size)
        assertEquals("acct-0001", prefs().pairedAccount)
    }

    // ---- another account where the phone can be on the wrong one (owner kb PHN-842) ------------------------------

    private fun keptAccount() = MemoryAccountStore(SignedIn(INVENTED_ACCOUNT, Tokens("fa1_invented-access", 3_600_000L, "fr1_invented-refresh")))

    // decision: PHN-842, PHN-843
    @Test
    fun use_another_account_on_the_phone_first_page_signs_out_at_once_with_its_pending_state() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        val store = keptAccount()
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        val vm = vm(testAccount(http, store, { now }, io = Dispatchers.IO))
        session.unlocked(); idle()
        assertEquals("signed in, no Mac: the phone-first page", Gate.PAIR, vm.ui.value.gate)
        assertFalse(vm.pairNow.value)

        // the site holds its answer: the tap's pending state shows at once, on the same page, nothing cleared yet
        val held = CountDownLatch(1)
        http.gate = held
        vm.signOut()
        eventually { http.requests.isNotEmpty() }
        assertTrue(vm.ui.value.signingOut)
        assertEquals(Gate.PAIR, vm.ui.value.gate)
        assertNotNull("the site is told first, with the tokens still kept", store.kept)
        // a second tap meanwhile is never a silent no-op
        vm.signOut(); idle()
        assertEquals(Note(R.string.still_working), vm.ui.value.note)

        held.countDown()
        eventually { !vm.ui.value.signingOut }
        assertEquals(listOf("/api/app/signout"), http.paths())
        assertTrue("the site revokes this sign-in", http.requests.single().body!!.contains("fr1_invented-refresh"))
        assertNull(store.kept)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        assertNull("the site was told: nothing to say", vm.ui.value.account.note)

        // the sign-in page again: Continue with Google opens the site's Google start (the site asks Google for its
        // account chooser, prompt=select_account)
        vm.signIn(Provider.GOOGLE)
        eventually { opened.isNotEmpty() }
        assertTrue(opened.single().startsWith("${Site.URL}/api/app/start?provider=google&platform=android"))
    }

    // decision: PHN-842, PHN-843
    @Test
    fun use_another_account_with_the_site_unreachable_still_signs_out_here_and_says_so() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        val store = keptAccount()
        val vm = vm(signedIn(store))
        session.unlocked(); idle()
        assertEquals(Gate.PAIR, vm.ui.value.gate)
        http.offline = true
        vm.signOut()
        eventually { !vm.ui.value.signingOut }
        assertEquals("tried", listOf("/api/app/signout"), http.paths())
        assertNull("signed out here all the same: never stuck", store.kept)
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        assertEquals(Note(R.string.signout_unreachable), vm.ui.value.account.note)
        // the next sign-in starts afresh
        http.offline = false
        vm.signIn(Provider.GOOGLE); idle()
        assertNull(vm.ui.value.account.note)
        assertEquals(1, opened.size)
    }

    // decision: PHN-842, PHN-828
    @Test
    fun the_macs_refusal_of_this_account_offers_another_account_and_signing_out_clears_it() {
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        http.reply("/api/app/assertion", 200, """{"assertion":"fpa1.invented-payload.invented-signature","expires_in":300}""")
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        val vm = vm(signedIn())
        session.unlocked(); idle()
        vm.pairNow(true)
        vm.pair("FERB-INVENTED"); idle()
        assertFalse(vm.ui.value.pairAccountRefused)
        val words = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."
        phone.link.value = FakePhone.status(LinkPhase.STOPPED, paired = false, problem = LinkProblem("account_mismatch", words, null))
        assertEquals(Note(R.string.err_mac_said, words), vm.ui.value.pairError)
        assertTrue("its words come with another account", vm.ui.value.pairAccountRefused)
        // a wrong code is no account's matter
        vm.clearPairError()
        assertFalse(vm.ui.value.pairAccountRefused)
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        vm.pair("FERB-INVENTED"); idle()
        phone.link.value = FakePhone.status(LinkPhase.WAITING, paired = false, problem = LinkProblem("pair_secret_wrong", "The code is wrong.", null))
        assertEquals(Note(R.string.pair_wrong_code), vm.ui.value.pairError)
        assertFalse(vm.ui.value.pairAccountRefused)
        // the Mac signed out: the same way out (as on the iPhone)
        phone.link.value = FakePhone.status(LinkPhase.IDLE, paired = false)
        vm.pair("FERB-INVENTED"); idle()
        phone.link.value = FakePhone.status(LinkPhase.STOPPED, paired = false,
            problem = LinkProblem("mac_signed_out", "FERB on this Mac isn't signed in yet. Sign in on the Mac, then scan a new code.", null))
        assertTrue(vm.ui.value.pairAccountRefused)

        vm.signOut()
        eventually { !vm.ui.value.signingOut }
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        assertNull("the refusal goes with the account", vm.ui.value.pairError)
        assertFalse(vm.ui.value.pairAccountRefused)
        assertFalse("signed in again, the phone-first page comes first", vm.pairNow.value)
    }

    // decision: PHN-842, PHN-828
    @Test
    fun a_phone_the_mac_removed_over_its_account_can_use_another_account() {
        legacyPairing()
        val words = "This phone is signed in as someone@example.com, but FERB on this Mac is signed in as other@example.com. Sign in with the same account on both."
        phone.bindScript.add(com.ferb.core.Outcome.Failed(com.ferb.mobile.FerbException.Refused(words, "account_mismatch")))
        http.reply("/api/app/signout", 200, """{"ok":true}""")
        val vm = vm(signedIn())
        session.unlocked()
        eventually { vm.ui.value.gate == Gate.REMOVED }
        assertEquals(Note(R.string.err_mac_said, words), vm.ui.value.removedNote)
        vm.signOut()
        eventually { !vm.ui.value.signingOut }
        assertTrue("the Mac is forgotten here", "forget" in phone.calls)
        assertTrue(http.paths().contains("/api/app/signout"))
        assertEquals(Gate.SIGN_IN, vm.ui.value.gate)
        assertNull(vm.ui.value.removedNote)
    }
}
