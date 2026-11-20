package org.andromeda.link.ui.theme

import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.invalidateDraw
import kotlinx.coroutines.launch

/** The pressed state as a wash over the content instead of a ripple: two steps up the fill ladder (fill_2). */
data class PressHighlight(
    val wash: Color,
) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): DelegatableNode = Node(interactionSource, wash)

    private class Node(
        private val source: InteractionSource,
        private val wash: Color,
    ) : Modifier.Node(),
        DrawModifierNode {
        private var pressed = false

        override fun onAttach() {
            coroutineScope.launch {
                source.interactions.collect { interaction ->
                    pressed =
                        when (interaction) {
                            is PressInteraction.Press -> true
                            is PressInteraction.Release, is PressInteraction.Cancel -> false
                            else -> pressed
                        }
                    invalidateDraw()
                }
            }
        }

        override fun ContentDrawScope.draw() {
            drawContent()
            if (pressed) drawRect(wash)
        }
    }
}
