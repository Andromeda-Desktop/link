// Generated from protocol/design-tokens.toml by protocol/gen-design-tokens.py. Do not edit.
package org.andromeda.link.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

object Tok {
    object Font {
        val family = "Inter Variable"
        val mono = "Fira Code, monospace"
    }

    @Immutable
    data class TypeToken(
        val size: Float,
        val line: Float,
        val weight: Int,
        val tracking: Float,
    )

    object Type {
        val caption =
            TypeToken(
                size = 11.0f,
                line = 16.0f,
                weight = 500,
                tracking = 0.0048f,
            )
        val footnote =
            TypeToken(
                size = 12.0f,
                line = 16.0f,
                weight = 400,
                tracking = 0.0005f,
            )
        val body =
            TypeToken(
                size = 13.0f,
                line = 16.0f,
                weight = 400,
                tracking = -0.0032f,
            )
        val paragraph =
            TypeToken(
                size = 13.0f,
                line = 20.0f,
                weight = 400,
                tracking = -0.0032f,
            )
        val headline =
            TypeToken(
                size = 13.0f,
                line = 16.0f,
                weight = 600,
                tracking = -0.0032f,
            )
        val title3 =
            TypeToken(
                size = 15.0f,
                line = 20.0f,
                weight = 600,
                tracking = -0.0088f,
            )
        val title2 =
            TypeToken(
                size = 17.0f,
                line = 24.0f,
                weight = 600,
                tracking = -0.0128f,
            )
        val title1 =
            TypeToken(
                size = 22.0f,
                line = 28.0f,
                weight = 600,
                tracking = 0.0f,
            )
        val large =
            TypeToken(
                size = 28.0f,
                line = 36.0f,
                weight = 600,
                tracking = 0.0f,
            )
        val display =
            TypeToken(
                size = 34.0f,
                line = 40.0f,
                weight = 500,
                tracking = 0.0f,
            )
        val mono =
            TypeToken(
                size = 13.0f,
                line = 18.0f,
                weight = 450,
                tracking = 0.0f,
            )
        val hero =
            TypeToken(
                size = 80.0f,
                line = 80.0f,
                weight = 300,
                tracking = 0.0f,
            )
    }

    object TypeRules {
        val glassWeightAdd = 100
        val textScaleS = 0.92f
        val textScaleL = 1.15f
        val textScaleXl = 1.3f
    }

    @Immutable
    data class SchemeToken(
        val label1: Color,
        val label2: Color,
        val label3: Color,
        val fill1: Color,
        val fill2: Color,
        val fill3: Color,
        val separator: Color,
        val stroke: Color,
        val edge: Color,
        val lens: Color,
        val lensEdge: Color,
        val tintThin: Color,
        val tintRegular: Color,
        val tintThick: Color,
        val solidThin: Color,
        val solidRegular: Color,
        val solidThick: Color,
        val accent: Color,
        val accentText: Color,
        val onAccent: Color,
        val danger: Color,
        val dangerText: Color,
        val warning: Color,
        val success: Color,
        val sliderFill: Color,
        val onSliderFill: Color,
        val sliderTrack: Color,
        val sliderEdge: Color,
        val glassCard: Color,
        val glassCardEdge: Color,
        val scrim: Color,
        val shadow: Color,
        val window: Color,
        val view: Color,
        val card: Color,
        val header: Color,
        val popover: Color,
        val sidebar: Color,
        val termRed: Color,
        val termGreen: Color,
        val termYellow: Color,
        val termBlue: Color,
        val termMagenta: Color,
        val termCyan: Color,
        val termFindHit: Color,
        val termFindHitCurrent: Color,
        val termFindRing: Color,
        val termCommandBand: Color,
        val diffAdd: Color,
        val diffDel: Color,
    )

    object Scheme {
        val light =
            SchemeToken(
                label1 = Color(0xEB141418),
                label2 = Color(0xB8141418),
                label3 = Color(0x7A141418),
                fill1 = Color(0x0A000000),
                fill2 = Color(0x14000000),
                fill3 = Color(0x1F000000),
                separator = Color(0x14000000),
                stroke = Color(0x1F000000),
                edge = Color(0xBFFFFFFF),
                lens = Color(0xEBFFFFFF),
                lensEdge = Color(0x00000000),
                tintThin = Color(0x8CF6F6FA),
                tintRegular = Color(0x57FFFFFF),
                tintThick = Color(0xE0FFFFFF),
                solidThin = Color(0xFFECECF1),
                solidRegular = Color(0xFFF2F2F6),
                solidThick = Color(0xFFF8F8FA),
                accent = Color(0xFF5147E5),
                accentText = Color(0xFF5147E5),
                onAccent = Color(0xFFFFFFFF),
                danger = Color(0xFFD93025),
                dangerText = Color(0xFFC5221F),
                warning = Color(0xFFB45309),
                success = Color(0xFF1A7F37),
                sliderFill = Color(0x1A141418),
                onSliderFill = Color(0xEB141418),
                sliderTrack = Color(0x0F000000),
                sliderEdge = Color(0x00000000),
                glassCard = Color(0x47FFFFFF),
                glassCardEdge = Color(0x14000000),
                scrim = Color(0x33000000),
                shadow = Color(0xFF141428),
                window = Color(0xFFF2F2F6),
                view = Color(0xFFFFFFFF),
                card = Color(0xFFFFFFFF),
                header = Color(0xFFECECF1),
                popover = Color(0xFFF8F8FA),
                sidebar = Color(0xFFECECF1),
                termRed = Color(0xFFA92321),
                termGreen = Color(0xFF146720),
                termYellow = Color(0xFF715403),
                termBlue = Color(0xFF2F56A1),
                termMagenta = Color(0xFF814085),
                termCyan = Color(0xFF06626B),
                termFindHit = Color(0x47F5A524),
                termFindHitCurrent = Color(0x9EF5A524),
                termFindRing = Color(0xFFC26A00),
                termCommandBand = Color(0x09141418),
                diffAdd = Color(0x1A146720),
                diffDel = Color(0x17A92321),
            )
        val dark =
            SchemeToken(
                label1 = Color(0xEBF4F4F6),
                label2 = Color(0x9EF4F4F6),
                label3 = Color(0x61F4F4F6),
                fill1 = Color(0x0FFFFFFF),
                fill2 = Color(0x1AFFFFFF),
                fill3 = Color(0x29FFFFFF),
                separator = Color(0x14FFFFFF),
                stroke = Color(0x14FFFFFF),
                edge = Color(0x1FFFFFFF),
                lens = Color(0x24FFFFFF),
                lensEdge = Color(0x2EFFFFFF),
                tintThin = Color(0x8C38383F),
                tintRegular = Color(0xB30B0B0F),
                tintThick = Color(0xEB202024),
                solidThin = Color(0xFF1C1C20),
                solidRegular = Color(0xFF222226),
                solidThick = Color(0xFF28282C),
                accent = Color(0xFF6159F5),
                accentText = Color(0xFF8C85FF),
                onAccent = Color(0xFFFFFFFF),
                danger = Color(0xFFD93025),
                dangerText = Color(0xFFFF6B5E),
                warning = Color(0xFFF5A524),
                success = Color(0xFF3FB950),
                sliderFill = Color(0x3DFFFFFF),
                onSliderFill = Color(0xEBFFFFFF),
                sliderTrack = Color(0x1AFFFFFF),
                sliderEdge = Color(0x00000000),
                glassCard = Color(0x0FFFFFFF),
                glassCardEdge = Color(0x00000000),
                scrim = Color(0x66000000),
                shadow = Color(0xFF000000),
                window = Color(0xFF1E1E22),
                view = Color(0xFF18181B),
                card = Color(0xFF28282C),
                header = Color(0xFF222226),
                popover = Color(0xFF28282C),
                sidebar = Color(0xFF1C1C20),
                termRed = Color(0xFFFF7C70),
                termGreen = Color(0xFF68B56C),
                termYellow = Color(0xFFCA9D33),
                termBlue = Color(0xFF77A4F6),
                termMagenta = Color(0xFFD28CD6),
                termCyan = Color(0xFF03B5C3),
                termFindHit = Color(0x38F5A524),
                termFindHitCurrent = Color(0x80F5A524),
                termFindRing = Color(0xFFF5A524),
                termCommandBand = Color(0x0BFFFFFF),
                diffAdd = Color(0x1F7FD88F),
                diffDel = Color(0x1FFF8F87),
            )
    }

    @Immutable
    data class ElevationToken(
        val keyY: Float,
        val keyBlur: Float,
        val keyLight: Float,
        val keyDark: Float,
        val contactY: Float,
        val contactBlur: Float,
        val contactLight: Float,
        val contactDark: Float,
    )

    object Elevation {
        val e1 =
            ElevationToken(
                keyY = 1.0f,
                keyBlur = 2.0f,
                keyLight = 0.1f,
                keyDark = 0.3f,
                contactY = 0.0f,
                contactBlur = 0.0f,
                contactLight = 0.0f,
                contactDark = 0.0f,
            )
        val e2 =
            ElevationToken(
                keyY = 9.0f,
                keyBlur = 24.0f,
                keyLight = 0.1f,
                keyDark = 0.15f,
                contactY = 1.0f,
                contactBlur = 6.0f,
                contactLight = 0.05f,
                contactDark = 0.1f,
            )
        val e3 =
            ElevationToken(
                keyY = 12.0f,
                keyBlur = 32.0f,
                keyLight = 0.16f,
                keyDark = 0.4f,
                contactY = 2.0f,
                contactBlur = 8.0f,
                contactLight = 0.08f,
                contactDark = 0.24f,
            )
        val e4 =
            ElevationToken(
                keyY = 24.0f,
                keyBlur = 64.0f,
                keyLight = 0.22f,
                keyDark = 0.5f,
                contactY = 2.0f,
                contactBlur = 8.0f,
                contactLight = 0.1f,
                contactDark = 0.3f,
            )
    }

    @Immutable
    data class SpringToken(
        val stiffness: Float,
        val damping: Float,
    )

    object Spring {
        val snap =
            SpringToken(
                stiffness = 584.0f,
                damping = 0.85f,
            )
        val trail =
            SpringToken(
                stiffness = 341.5f,
                damping = 0.8f,
            )
        val move =
            SpringToken(
                stiffness = 322.3f,
                damping = 0.9f,
            )
        val open =
            SpringToken(
                stiffness = 584.0f,
                damping = 0.9f,
            )
        val zoom =
            SpringToken(
                stiffness = 341.5f,
                damping = 0.8f,
            )
        val close =
            SpringToken(
                stiffness = 685.4f,
                damping = 1.0f,
            )
        val morph =
            SpringToken(
                stiffness = 186.6f,
                damping = 0.82f,
            )
        val collapse =
            SpringToken(
                stiffness = 223.8f,
                damping = 0.8f,
            )
        val page =
            SpringToken(
                stiffness = 385.5f,
                damping = 0.95f,
            )
        val press =
            SpringToken(
                stiffness = 2741.6f,
                damping = 1.0f,
            )
        val release =
            SpringToken(
                stiffness = 385.5f,
                damping = 0.75f,
            )
        val fade =
            SpringToken(
                stiffness = 1542.1f,
                damping = 1.0f,
            )
        val follow =
            SpringToken(
                stiffness = 2741.6f,
                damping = 1.0f,
            )
        val fadeOut =
            SpringToken(
                stiffness = 3947.8f,
                damping = 1.0f,
            )
    }

    object Motion {
        val contentGate = 0.35f
        val faceGate = 0.5f
        val returnFaceGate = 0.7f
        val overviewPieceScale = 0.86f
        val staggerMs = 8
        val staggerMax = 6
        val faceScale = 0.92f
        val pourXShare = 0.15f
        val pourYShare = 0.2f
        val growMinScale = 0.05f
        val faceBlur = 8.0f
        val pressScaleSmall = 0.96f
        val pressScaleMedium = 0.98f
        val slop = 8.0f
        val rubberBand = 0.55f
        val projectionDecay = 0.998f
        val projectionDecayStepped = 0.997f
        val swipeDim = 0.2f
        val openVelocity = 300.0f
        val loadingDelayMs = 200
        val loadingMinMs = 400
        val caretBlinkMs = 530
        val spinnerPeriodMs = 800
        val spinnerArc = 270.0f
    }

    object Radius {
        val smoothing = 0.6f
        val r4 = 4.0f
        val r8 = 8.0f
        val r12 = 12.0f
        val r16 = 16.0f
        val r20 = 20.0f
        val r24 = 24.0f
        val r28 = 28.0f
        val pill = 999.0f
    }

    object Space {
        val p2 = 2.0f
        val p4 = 4.0f
        val p8 = 8.0f
        val p12 = 12.0f
        val p16 = 16.0f
        val p20 = 20.0f
        val p24 = 24.0f
        val p32 = 32.0f
        val p40 = 40.0f
        val p48 = 48.0f
        val p64 = 64.0f
        val chromeGap = 8.0f
    }

    object Size {
        val controlS = 24.0f
        val controlM = 32.0f
        val controlL = 40.0f
        val controlXl = 48.0f
        val rowCompact = 32.0f
        val row = 40.0f
        val rowTwoCompact = 48.0f
        val rowTwo = 56.0f
        val symbolInline = 12.0f
        val symbol = 16.0f
        val symbolL = 20.0f
        val keycap = 20.0f
        val hitMin = 24.0f
        val hitTouch = 40.0f
    }

    object Phone {
        val hit = 48.0f
        val symbol = 22.0f
        val chip = 36.0f
        val pillPad = 14.0f
        val valueMax = 160.0f
        val chipTint = 0.12f
        val glyph = 52.0f
        val glyphTint = 0.14f
        val identity = 64.0f
        val fixChip = 22.0f
        val slider = 52.0f
        val mediaArt = 240.0f
        val dockInset = 28.0f
        val fadeDock = 132.0f
        val offAlpha = 0.38f
        val grab = 36.0f
        val deskGlyph = 40.0f
        val roundKeyL = 60.0f
        val padDot = 1.4f
        val rail = 28.0f
        val keyS = 44.0f
        val composer = 52.0f
        val prevZone = 120.0f
        val timerScale = 2.6f
        val planeKey = 0.18f
        val picture = 0.54f
        val pointer = 26.0f
        val minimap = 104.0f
        val holdRing = 64.0f
        val followZoom = 2.5f
        val followZoomWide = 1.6f
        val followBand = 0.6f
        val maxZoom = 5.0f
        val heroGlyph = 80.0f
        val heroSymbol = 40.0f
        val orb = 120.0f
        val orbRing = 0.08f
        val orbTint = 0.22f
        val codeCell = 64.0f
        val finderCorner = 44.0f
        val finderInset = 0.22f
        val pickRing = 3.0f
        val pickBadge = 24.0f
        val pickBadgeRing = 1.5f
        val pickMaxTop = 64.0f
        val hero = 0.5f
        val heroScene = 0.28f
        val heroIdentity = 0.6f
        val sheetRadius = 32.0f
        val levelGap = 12.0f
    }
}
