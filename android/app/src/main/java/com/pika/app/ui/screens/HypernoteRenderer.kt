package com.pika.app.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckBox
import androidx.compose.material.icons.filled.CheckBoxOutlineBlank
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import com.pika.app.rust.HypernoteData
import com.pika.app.rust.HypernoteResponseTally
import com.pika.app.ui.Avatar
import org.json.JSONArray
import org.json.JSONObject

private data class HypernoteAstNode(
    val type: String,
    val value: String? = null,
    val children: List<HypernoteAstNode> = emptyList(),
    val level: Int? = null,
    val url: String? = null,
    val lang: String? = null,
    val name: String? = null,
    val attributes: List<HypernoteAstAttribute> = emptyList(),
)

private data class HypernoteAstAttribute(
    val name: String,
    val type: String? = null,
    val value: String? = null,
)

@Composable
internal fun HypernoteRenderer(
    messageId: String,
    hypernote: HypernoteData,
    onAction: (actionName: String, messageId: String, form: Map<String, String>) -> Unit,
    modifier: Modifier = Modifier,
) {
    val root = remember(hypernote.astJson) { parseHypernoteAst(hypernote.astJson) }
    val talliesByAction =
        remember(hypernote.responseTallies) {
            hypernote.responseTallies.associateBy(HypernoteResponseTally::action)
        }
    val interactionState =
        remember(messageId, hypernote.defaultState) {
            mutableStateMapOf<String, String>().apply {
                putAll(parseDefaultState(hypernote.defaultState))
            }
        }
    var localSubmittedAction by remember(messageId) { mutableStateOf<String?>(null) }
    val selectedAction = hypernote.myResponse ?: localSubmittedAction
    val isSubmitted = selectedAction != null

    Surface(
        modifier = modifier.widthIn(max = 300.dp),
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier =
                Modifier
                    .alpha(if (isSubmitted) 0.84f else 1f)
                    .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (root == null) {
                Text(
                    text = "Failed to parse hypernote",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                root.children.forEach { child ->
                    HypernoteNode(
                        node = child,
                        interactionState = interactionState,
                        talliesByAction = talliesByAction,
                        selectedAction = selectedAction,
                        isSubmitted = isSubmitted,
                        messageId = messageId,
                        onAction = { actionName, form ->
                            localSubmittedAction = actionName
                            onAction(actionName, messageId, form)
                        },
                    )
                }
            }

            if (hypernote.responders.isNotEmpty()) {
                val avatarOutlineColor = MaterialTheme.colorScheme.surfaceContainerHigh
                Row(
                    modifier = Modifier.padding(top = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy((-6).dp),
                ) {
                    hypernote.responders.take(5).forEach { responder ->
                        Box {
                            Avatar(
                                name = responder.name,
                                npub = responder.npub,
                                pictureUrl = responder.pictureUrl,
                                size = 20.dp,
                            )
                            Box(
                                modifier =
                                    Modifier
                                        .size(20.dp)
                                        .clip(CircleShape)
                                        .drawBehind {
                                            drawCircle(
                                                color = avatarOutlineColor,
                                                style = Stroke(width = 1.5.dp.toPx()),
                                            )
                                        },
                            )
                        }
                    }
                    if (hypernote.responders.size > 5) {
                        Text(
                            text = "+${hypernote.responders.size - 5}",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(start = 4.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun HypernoteNode(
    node: HypernoteAstNode,
    interactionState: MutableMap<String, String>,
    talliesByAction: Map<String, HypernoteResponseTally>,
    selectedAction: String?,
    isSubmitted: Boolean,
    messageId: String,
    onAction: (actionName: String, form: Map<String, String>) -> Unit,
) {
    when (node.type) {
        "heading" -> {
            val style =
                when (node.level ?: 1) {
                    1 -> MaterialTheme.typography.titleLarge
                    2 -> MaterialTheme.typography.titleMedium
                    3 -> MaterialTheme.typography.titleSmall
                    else -> MaterialTheme.typography.titleSmall
                }
            Text(
                text = inlineText(node.children),
                style = style,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        "paragraph" -> {
            if (hasOnlyInlineChildren(node.children)) {
                val markdown = buildInlineAnnotated(node.children)
                if (markdown.text.isNotBlank()) {
                    Text(
                        text = markdown,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                }
            } else {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    node.children.forEach { child ->
                        HypernoteNode(
                            node = child,
                            interactionState = interactionState,
                            talliesByAction = talliesByAction,
                            selectedAction = selectedAction,
                            isSubmitted = isSubmitted,
                            messageId = messageId,
                            onAction = onAction,
                        )
                    }
                }
            }
        }

        "strong", "emphasis", "text", "code_inline", "link" -> {
            val markdown = buildInlineAnnotated(listOf(node))
            if (markdown.text.isNotBlank()) {
                Text(
                    text = markdown,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                )
            }
        }

        "code_block" -> {
            Surface(
                shape = RoundedCornerShape(8.dp),
                color = MaterialTheme.colorScheme.surfaceContainerHighest,
            ) {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    node.lang?.takeIf { it.isNotBlank() }?.let { language ->
                        Text(
                            text = language,
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp),
                        )
                    }
                    Text(
                        text = node.value.orEmpty(),
                        style =
                            MaterialTheme.typography.bodySmall.copy(
                                fontFamily = FontFamily.Monospace,
                            ),
                        color = MaterialTheme.colorScheme.onSurface,
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp),
                    )
                }
            }
        }

        "image" -> {
            val model = node.url
            if (!model.isNullOrBlank()) {
                AsyncImage(
                    model = model,
                    contentDescription = "Hypernote image",
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(8.dp)),
                )
            }
        }

        "list_unordered", "list_ordered" -> {
            val ordered = node.type == "list_ordered"
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                node.children.forEachIndexed { index, child ->
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.Top,
                    ) {
                        Text(
                            text = if (ordered) "${index + 1}." else "•",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Column(
                            modifier = Modifier.weight(1f),
                            verticalArrangement = Arrangement.spacedBy(2.dp),
                        ) {
                            if (child.children.isNotEmpty()) {
                                child.children.forEach { grandChild ->
                                    HypernoteNode(
                                        node = grandChild,
                                        interactionState = interactionState,
                                        talliesByAction = talliesByAction,
                                        selectedAction = selectedAction,
                                        isSubmitted = isSubmitted,
                                        messageId = messageId,
                                        onAction = onAction,
                                    )
                                }
                            } else {
                                HypernoteNode(
                                    node = child,
                                    interactionState = interactionState,
                                    talliesByAction = talliesByAction,
                                    selectedAction = selectedAction,
                                    isSubmitted = isSubmitted,
                                    messageId = messageId,
                                    onAction = onAction,
                                )
                            }
                        }
                    }
                }
            }
        }

        "blockquote" -> {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(
                    modifier =
                        Modifier
                            .widthIn(min = 3.dp, max = 3.dp)
                            .height(26.dp)
                            .clip(RoundedCornerShape(2.dp))
                            .background(MaterialTheme.colorScheme.outline.copy(alpha = 0.6f)),
                )
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    node.children.forEach { child ->
                        HypernoteNode(
                            node = child,
                            interactionState = interactionState,
                            talliesByAction = talliesByAction,
                            selectedAction = selectedAction,
                            isSubmitted = isSubmitted,
                            messageId = messageId,
                            onAction = onAction,
                        )
                    }
                }
            }
        }

        "hr" -> {
            HorizontalDivider()
        }

        "hard_break" -> {
            Spacer(Modifier.height(4.dp))
        }

        "mdx_jsx_element", "mdx_jsx_self_closing" -> {
            HypernoteJsxNode(
                node = node,
                interactionState = interactionState,
                talliesByAction = talliesByAction,
                selectedAction = selectedAction,
                isSubmitted = isSubmitted,
                messageId = messageId,
                onAction = onAction,
            )
        }

        else -> {
            if (node.children.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    node.children.forEach { child ->
                        HypernoteNode(
                            node = child,
                            interactionState = interactionState,
                            talliesByAction = talliesByAction,
                            selectedAction = selectedAction,
                            isSubmitted = isSubmitted,
                            messageId = messageId,
                            onAction = onAction,
                        )
                    }
                }
            } else if (!node.value.isNullOrBlank()) {
                Text(
                    text = node.value.orEmpty(),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                )
            }
        }
    }
}

@Composable
private fun HypernoteJsxNode(
    node: HypernoteAstNode,
    interactionState: MutableMap<String, String>,
    talliesByAction: Map<String, HypernoteResponseTally>,
    selectedAction: String?,
    isSubmitted: Boolean,
    messageId: String,
    onAction: (actionName: String, form: Map<String, String>) -> Unit,
) {
    val attrs = remember(node.attributes) {
        node.attributes.associate { attr -> attr.name to attr.value.orEmpty() }
    }
    when (node.name.orEmpty()) {
        "Card" -> {
            Surface(
                shape = RoundedCornerShape(12.dp),
                color = MaterialTheme.colorScheme.surfaceContainerHighest,
            ) {
                Column(
                    modifier = Modifier.padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    node.children.forEach { child ->
                        HypernoteNode(
                            node = child,
                            interactionState = interactionState,
                            talliesByAction = talliesByAction,
                            selectedAction = selectedAction,
                            isSubmitted = isSubmitted,
                            messageId = messageId,
                            onAction = onAction,
                        )
                    }
                }
            }
        }

        "VStack" -> {
            val gap = attrs["spacing"]?.toIntOrNull() ?: attrs["gap"]?.toIntOrNull() ?: 8
            Column(verticalArrangement = Arrangement.spacedBy(gap.dp)) {
                node.children.forEach { child ->
                    HypernoteNode(
                        node = child,
                        interactionState = interactionState,
                        talliesByAction = talliesByAction,
                        selectedAction = selectedAction,
                        isSubmitted = isSubmitted,
                        messageId = messageId,
                        onAction = onAction,
                    )
                }
            }
        }

        "HStack" -> {
            val gap = attrs["spacing"]?.toIntOrNull() ?: attrs["gap"]?.toIntOrNull() ?: 8
            Row(horizontalArrangement = Arrangement.spacedBy(gap.dp), verticalAlignment = Alignment.CenterVertically) {
                node.children.forEach { child ->
                    HypernoteNode(
                        node = child,
                        interactionState = interactionState,
                        talliesByAction = talliesByAction,
                        selectedAction = selectedAction,
                        isSubmitted = isSubmitted,
                        messageId = messageId,
                        onAction = onAction,
                    )
                }
            }
        }

        "Heading" -> {
            Text(
                text = inlineText(node.children),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        "Body" -> {
            Text(
                text = inlineText(node.children),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        "Caption" -> {
            Text(
                text = inlineText(node.children),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        "TextInput" -> {
            val fieldName = attrs["name"].orEmpty().ifBlank { "field" }
            val placeholder = attrs["placeholder"].orEmpty()
            OutlinedTextField(
                value = interactionState[fieldName].orEmpty(),
                onValueChange = { interactionState[fieldName] = it },
                enabled = !isSubmitted,
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
                placeholder = {
                    if (placeholder.isNotBlank()) {
                        Text(text = placeholder)
                    }
                },
            )
        }

        "SubmitButton" -> {
            val actionName = attrs["action"].orEmpty().ifBlank { "submit" }
            val variant = attrs["variant"].orEmpty().ifBlank { "primary" }
            val isSelected = selectedAction == actionName
            val isUnselected = isSubmitted && !isSelected
            val useFilled =
                when (variant) {
                    "secondary" -> isSelected
                    else -> !isSubmitted || isSelected
                }
            val tally = talliesByAction[actionName]

            val label: @Composable () -> Unit = {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.Center,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (isSelected) {
                        Icon(
                            imageVector = Icons.Default.Check,
                            contentDescription = null,
                            modifier = Modifier.size(14.dp),
                        )
                        Spacer(Modifier.size(6.dp))
                    }
                    Text(text = inlineText(node.children).ifBlank { actionName })
                    if (tally != null) {
                        Spacer(Modifier.size(6.dp))
                        Text(
                            text = tally.count.toString(),
                            style = MaterialTheme.typography.labelMedium,
                        )
                    }
                }
            }

            if (useFilled) {
                Button(
                    onClick = { onAction(actionName, interactionState.toMap()) },
                    enabled = !isSubmitted,
                    colors =
                        if (variant == "danger") {
                            ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error)
                        } else {
                            ButtonDefaults.buttonColors()
                        },
                    modifier = Modifier.fillMaxWidth().alpha(if (isUnselected) 0.5f else 1f),
                ) {
                    label()
                }
            } else {
                OutlinedButton(
                    onClick = { onAction(actionName, interactionState.toMap()) },
                    enabled = !isSubmitted,
                    modifier = Modifier.fillMaxWidth().alpha(if (isUnselected) 0.5f else 1f),
                ) {
                    label()
                }
            }
        }

        "ChecklistItem" -> {
            val fieldName = attrs["name"].orEmpty().ifBlank { "item" }
            val defaultChecked = attrs.containsKey("checked")
            LaunchedEffect(fieldName, defaultChecked) {
                if (!interactionState.containsKey(fieldName)) {
                    interactionState[fieldName] = if (defaultChecked) "true" else "false"
                }
            }
            val isChecked = interactionState[fieldName] == "true"
            Row(
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(enabled = !isSubmitted) {
                            interactionState[fieldName] = if (isChecked) "false" else "true"
                        }
                        .padding(vertical = 4.dp),
                verticalAlignment = Alignment.Top,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    imageVector = if (isChecked) Icons.Default.CheckBox else Icons.Default.CheckBoxOutlineBlank,
                    contentDescription = null,
                    tint =
                        if (isChecked) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    modifier = Modifier.size(20.dp),
                )
                Text(
                    text = inlineText(node.children),
                    style = MaterialTheme.typography.bodyMedium,
                    color =
                        if (isChecked) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.onSurface
                        },
                    textDecoration = if (isChecked) TextDecoration.LineThrough else TextDecoration.None,
                    modifier = Modifier.weight(1f),
                )
            }
        }

        else -> {
            val density = LocalDensity.current
            val outlineColor = MaterialTheme.colorScheme.outlineVariant
            Column(
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .drawBehind {
                            val strokeWidth = with(density) { 1.dp.toPx() }
                            val radius = with(density) { 8.dp.toPx() }
                            val pathEffect = PathEffect.dashPathEffect(floatArrayOf(8f, 8f), 0f)
                            drawRoundRect(
                                color = outlineColor,
                                style =
                                    Stroke(
                                        width = strokeWidth,
                                        cap = StrokeCap.Butt,
                                        pathEffect = pathEffect,
                                    ),
                                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
                            )
                        }
                        .padding(8.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                node.children.forEach { child ->
                    HypernoteNode(
                        node = child,
                        interactionState = interactionState,
                        talliesByAction = talliesByAction,
                        selectedAction = selectedAction,
                        isSubmitted = isSubmitted,
                        messageId = messageId,
                        onAction = onAction,
                    )
                }
            }
        }
    }
}

private fun parseHypernoteAst(astJson: String): HypernoteAstNode? =
    runCatching {
        val root = JSONObject(astJson)
        parseNode(root)
    }.getOrNull()

private fun parseNode(json: JSONObject): HypernoteAstNode {
    val childrenJson = json.optJSONArray("children")
    val attrsJson = json.optJSONArray("attributes")
    return HypernoteAstNode(
        type = json.optString("type"),
        value = json.optAnyAsString("value"),
        children = parseChildren(childrenJson),
        level = json.optInt("level").takeIf { json.has("level") },
        url = json.optAnyAsString("url"),
        lang = json.optAnyAsString("lang"),
        name = json.optAnyAsString("name"),
        attributes = parseAttributes(attrsJson),
    )
}

private fun parseChildren(children: JSONArray?): List<HypernoteAstNode> {
    if (children == null) return emptyList()
    val out = ArrayList<HypernoteAstNode>(children.length())
    for (i in 0 until children.length()) {
        val child = children.optJSONObject(i) ?: continue
        out += parseNode(child)
    }
    return out
}

private fun parseAttributes(attrs: JSONArray?): List<HypernoteAstAttribute> {
    if (attrs == null) return emptyList()
    val out = ArrayList<HypernoteAstAttribute>(attrs.length())
    for (i in 0 until attrs.length()) {
        val attr = attrs.optJSONObject(i) ?: continue
        val name = attr.optString("name")
        if (name.isBlank()) continue
        out +=
            HypernoteAstAttribute(
                name = name,
                type = attr.optAnyAsString("type"),
                value = attr.optAnyAsString("value"),
            )
    }
    return out
}

private fun JSONObject.optAnyAsString(key: String): String? {
    if (!has(key)) return null
    val raw = opt(key) ?: return null
    if (raw == JSONObject.NULL) return null
    return when (raw) {
        is String -> raw
        is Boolean -> if (raw) "true" else "false"
        is Number -> raw.toString()
        else -> raw.toString()
    }
}

private fun parseDefaultState(defaultState: String?): Map<String, String> {
    if (defaultState.isNullOrBlank()) return emptyMap()
    val obj = runCatching { JSONObject(defaultState) }.getOrNull() ?: return emptyMap()
    val out = LinkedHashMap<String, String>()
    val keys = obj.keys()
    while (keys.hasNext()) {
        val key = keys.next()
        val value = obj.opt(key)
        if (value == null || value == JSONObject.NULL) continue
        out[key] =
            when (value) {
                is String -> value
                is Boolean -> if (value) "true" else "false"
                is Number -> value.toString()
                else -> value.toString()
            }
    }
    return out
}

private fun hasOnlyInlineChildren(children: List<HypernoteAstNode>): Boolean {
    if (children.isEmpty()) return true
    val inlineTypes = setOf("text", "strong", "emphasis", "code_inline", "link", "hard_break")
    return children.all { it.type in inlineTypes }
}

private fun inlineText(children: List<HypernoteAstNode>): String =
    buildString {
        children.forEach { node ->
            append(inlineNodeText(node))
        }
    }

private fun inlineNodeText(node: HypernoteAstNode): String =
    when (node.type) {
        "text", "code_inline" -> node.value.orEmpty()
        "link" -> {
            val label = inlineText(node.children)
            if (label.isNotBlank()) label else node.url.orEmpty()
        }
        "hard_break" -> "\n"
        else -> {
            if (node.children.isNotEmpty()) {
                inlineText(node.children)
            } else {
                node.value.orEmpty()
            }
        }
    }

@Composable
private fun buildInlineAnnotated(children: List<HypernoteAstNode>): AnnotatedString {
    val textColor = MaterialTheme.colorScheme.onSurface
    val codeBackground = MaterialTheme.colorScheme.surfaceContainerHighest
    val linkColor = MaterialTheme.colorScheme.primary
    return remember(children, textColor, codeBackground, linkColor) {
        buildAnnotatedString {
            children.forEach { node ->
                appendInlineNode(this, node, linkColor, codeBackground)
            }
        }
    }
}

private fun appendInlineNode(
    builder: AnnotatedString.Builder,
    node: HypernoteAstNode,
    linkColor: Color,
    codeBackground: Color,
) {
    when (node.type) {
        "text" -> builder.append(node.value.orEmpty())
        "strong" -> {
            builder.pushStyle(SpanStyle(fontWeight = FontWeight.Bold))
            node.children.forEach { child -> appendInlineNode(builder, child, linkColor, codeBackground) }
            builder.pop()
        }
        "emphasis" -> {
            builder.pushStyle(SpanStyle(fontStyle = FontStyle.Italic))
            node.children.forEach { child -> appendInlineNode(builder, child, linkColor, codeBackground) }
            builder.pop()
        }
        "code_inline" -> {
            builder.pushStyle(
                SpanStyle(
                    fontFamily = FontFamily.Monospace,
                    background = codeBackground,
                ),
            )
            builder.append(node.value.orEmpty())
            builder.pop()
        }
        "link" -> {
            builder.pushStyle(
                SpanStyle(
                    color = linkColor,
                    textDecoration = TextDecoration.Underline,
                ),
            )
            val label = inlineText(node.children)
            if (label.isNotBlank()) {
                builder.append(label)
            } else {
                builder.append(node.url.orEmpty())
            }
            builder.pop()
        }
        "hard_break" -> builder.append('\n')
        else -> {
            if (node.children.isNotEmpty()) {
                node.children.forEach { child -> appendInlineNode(builder, child, linkColor, codeBackground) }
            } else {
                builder.append(node.value.orEmpty())
            }
        }
    }
}
