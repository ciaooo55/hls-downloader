package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class SiteRuleEditorTest {
    @Test
    fun json_rules_roundtrip_without_exposing_credentials() {
        val encoded = encodeSiteRules(listOf(SiteRuleDto(
            host = "cdn.example.test",
            enabled = true,
            speedLimitKib = 512,
            concurrency = 4,
            proxyMode = "direct",
            referer = "https://example.test/watch",
            origin = "https://example.test",
            credentialRef = "settings:site-rule:1234",
        )))
        val parsed = parseSiteRulesForEditor(encoded).single()
        assertEquals("cdn.example.test", parsed.host)
        assertEquals(512, parsed.speedLimitKib)
        assertEquals("direct", parsed.proxyMode)
        assertTrue(encoded.contains("credential_ref"))
        assertTrue(!encoded.contains("Cookie"))
    }

    @Test
    fun legacy_rules_are_upgraded_and_invalid_drafts_are_rejected() {
        val parsed = parseSiteRulesForEditor("cdn.test=speed:256,conn:2,origin:https://site.test")
        assertEquals(1, parsed.size)
        assertEquals(256, parsed.single().speedLimitKib)
        assertEquals(2, parsed.single().concurrency)
        assertNotNull(validateSiteRuleDrafts(listOf(SiteRuleDto(host = "bad/path"))))
        assertNotNull(validateSiteRuleDrafts(listOf(SiteRuleDto(host = "a.test"), SiteRuleDto(host = "A.TEST"))))
        assertNull(validateSiteRuleDrafts(listOf(SiteRuleDto(host = "a.test", origin = "https://site.test"))))
    }

    @Test
    fun request_header_lines_are_bounded_by_structure() {
        assertEquals(
            mapOf("Authorization" to "Bearer token", "X-Playback-Token" to "abc"),
            parseSiteRuleHeaders("Authorization: Bearer token\ninvalid\nX-Playback-Token: abc"),
        )
    }
}
