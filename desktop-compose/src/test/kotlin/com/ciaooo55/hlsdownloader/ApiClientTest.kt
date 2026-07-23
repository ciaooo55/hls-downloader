package com.ciaooo55.hlsdownloader

import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals

class ApiClientTest {
    private val json = Json

    @Test
    fun `keeps string API error details`() {
        assertEquals("下载链接无效", apiErrorMessage(json.parseToJsonElement("""{\"detail\":\"下载链接无效\"}"""), 422))
    }

    @Test
    fun `formats FastAPI validation arrays without throwing`() {
        val response = """{\"detail\":[{\"type\":\"missing\",\"loc\":[\"body\",\"url\"],\"msg\":\"Field required\"}]}"""
        assertEquals("Field required", apiErrorMessage(json.parseToJsonElement(response), 422))
    }

    @Test
    fun `falls back to HTTP status for an unstructured response`() {
        assertEquals("HTTP 403", apiErrorMessage(json.parseToJsonElement("{}"), 403))
    }
}
