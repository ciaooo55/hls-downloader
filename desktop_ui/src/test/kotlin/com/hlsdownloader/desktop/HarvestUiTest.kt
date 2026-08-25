package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals

class HarvestUiTest {
    private val links = listOf(
        HarvestCandidateUi("https://cdn.test/a.mp4", "a.mp4", "video", 8_000_000, "mp4"),
        HarvestCandidateUi("https://cdn.test/b.zip", "b.zip", "archive", 12_000, "zip"),
        HarvestCandidateUi("magnet:?xt=urn:btih:abc", "torrent", "torrent", 0, "torrent"),
    )

    @Test
    fun categoryCountsAndSizeFiltersMatchTheV3Workflow() {
        assertEquals(mapOf("all" to 3, "video" to 1, "archive" to 1, "torrent" to 1), harvestFilterCounts(links))
        assertEquals(listOf("a.mp4"), visibleHarvestLinks(links, "video", 0).map { it.filename })
        assertEquals(listOf("a.mp4"), visibleHarvestLinks(links, "all", 1024L * 1024L).map { it.filename })
    }

    @Test
    fun sizeProbeMergeKeepsUnprobedMetadataAndOrdering() {
        val merged = mergeHarvestSizes(links, mapOf(links[0].url to 9_000_000, links[1].url to 14_000))
        assertEquals(listOf("a.mp4", "b.zip", "torrent"), merged.map { it.filename })
        assertEquals(listOf("mp4", "zip", "torrent"), merged.map { it.extension })
        assertEquals(9_000_000, merged[0].size)
        assertEquals(14_000, merged[1].size)
        assertEquals(0, merged[2].size)
    }
}
