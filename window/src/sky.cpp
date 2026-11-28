#include "sky.h"

#include "app/app_window.h"
#include "core/blocking_pool.h"
#include "design_tokens.h"
#include "render/animation/motion_service.h"
#include "render/core/color.h"
#include "render/scene/galaxy_node.h"
#include "render/scene/input_area.h"
#include "ui/builders.h"
#include "ui/controls/box.h"

#include <algorithm>
#include <cmath>
#include <numbers>

namespace linkapp {

  namespace {
    constexpr float kTwoPi = 2.0F * std::numbers::pi_v<float>;

    // The band's left scrim (pages.html .band::after) at `strength`; s = UI scale.
    GalaxyScrim bandScrim(bool light, float strength, float s) {
      const Color edge = rgbaHex(light ? Tokens::componentLinkBandScrimLight : Tokens::componentLinkBandScrimDark);
      const Color mid = rgbaHex(light ? Tokens::componentLinkBandScrimLightMid : Tokens::componentLinkBandScrimDarkMid);
      return {.color = {edge.r, edge.g, edge.b}, .edge = edge.a, .mid = mid.a, .midX = Tokens::componentLinkBandScrimMid * s,
              .endX = Tokens::componentLinkBandScrimEnd * s, .strength = strength};
    }
    // Cameras of the handoff (mockups/link-desk-v2/handoff.yaml home, band).
    constexpr GalaxyCamera kHero{.yaw = 0.55F, .pitch = 0.42F, .dist = 3.0F, .sx = 0.12F, .sy = 0.12F};
    constexpr GalaxyCamera kBand{.yaw = 0.55F, .pitch = 0.3F, .dist = 1.15F, .sx = -0.18F, .sy = 0.3F};
    // Drag: rad per px; wheel: zoom factor per step; the Hero camera's reach.
    constexpr float kYawPerPx = 0.006F;
    constexpr float kPitchPerPx = 0.005F;
    constexpr float kPitchMin = 0.1F;
    constexpr float kPitchMax = 1.3F;
    constexpr float kZoomStep = 0.1F;
    constexpr float kDistMin = 1.6F;
    constexpr float kDistMax = 6.0F;
    // A wheel zoom counts as moving until the wheel rests this long; then the points redraw in full.
    constexpr float kWheelRestMs = 160.0F;
    // A star this close above the sky's bottom hides its leader (the sheet covers it).
    constexpr float kLeaderFloor = 30.0F;
    // A leader meets its card this far below the card's top at most (the card's first line).
    constexpr float kLeaderReach = 26.0F;
    // The band draws its leaders smaller and fainter than the hero (handoff.yaml StarLeaders).
    constexpr float kBandLeaderAlpha = 0.35F;
    constexpr float kBandHaloShare = 0.8F;
    constexpr float kBandDotShare = 0.875F;

    float lifeSpin(SkyLife life) {
      return life == SkyLife::Music ? Tokens::componentLinkSpinMusic : life != SkyLife::Still ? Tokens::componentLinkSpinBusy : 0.0F;
    }
    float lifeGlow(SkyLife life) {
      return life == SkyLife::Music ? Tokens::componentLinkGlowMusic : life != SkyLife::Still ? Tokens::componentLinkGlowBusy : 0.0F;
    }
    bool motionAllowed() { return MotionService::instance().enabled() && !MotionService::instance().reduced(); }

    std::array<float, 2> offsetIn(const Node* node, const Node* ancestor) {
      std::array<float, 2> at{};
      for (const Node* n = node; n != nullptr && n != ancestor; n = n->parent()) {
        at[0] += n->x() + n->translationX();
        at[1] += n->y() + n->translationY();
      }
      return at;
    }
  } // namespace

  // The sky's root: it places the leaders once the cards in front have their layout.
  class Sky::Layer final : public Node {
  public:
    explicit Layer(Sky& sky) : m_sky(sky) {}
    ~Layer() override {
      if (m_sky.m_layer != this) return;
      m_sky.stopLoop();
      m_sky.m_layer = nullptr;
      m_sky.m_galaxy = nullptr;
      m_sky.m_leaders.clear();
      m_sky.m_anchors.clear();
      m_sky.m_follower = nullptr;
    }
    Node* front = nullptr;
    Node* buttons = nullptr;

  protected:
    void doLayout(Renderer& renderer) override {
      Node::doLayout(renderer);
      if (buttons != nullptr) {
        const float edge = Tokens::spaceP12 * m_sky.m_scale;
        buttons->setPosition(width() - buttons->width() - edge, edge);
      }
      m_sky.placeLeaders();
    }

  private:
    Sky& m_sky;
  };

  Sky::Sky(AppWindow& window) : m_window(window) {}

  Sky::~Sky() {
    stopLoop();
    m_window.animations().cancelForOwner(&m_morph);
    m_window.animations().cancelForOwner(&m_arrivals);
    if (m_layer != nullptr) m_layer->front = nullptr;
    m_layer = nullptr;
  }

  void Sky::startPoints(Shape want) {
    if (m_building || (m_points != nullptr && m_pointsShape == want)) return;
    m_building = true;
    GalaxyShape shape;
    if (want.cool) shape.temp = 1900.0;
    if (want.forming) {
      // pages.html PAGE pair
      shape.seed = 41;
      shape.stars = 9000;
      shape.h2 = 60;
      shape.glob = 6;
    }
    BlockingPool::instance().run<std::shared_ptr<const GalaxyPoints>>(
        m_alive, [shape] { return std::make_shared<const GalaxyPoints>(buildGalaxy(shape)); },
        [this, want](std::shared_ptr<const GalaxyPoints> points) {
          m_building = false;
          m_points = std::move(points);
          m_pointsShape = want;
          if (m_galaxy != nullptr && want == Shape{.cool = m_look.cool, .forming = m_look.forming}) m_galaxy->setPoints(m_points);
          // The look may have changed while this one was built.
          startPoints({.cool = m_look.cool, .forming = m_look.forming});
        });
  }

  GalaxyCamera Sky::camera() const {
    GalaxyCamera c = kBand;
    if (m_look.mode == SkyMode::Full && m_look.camera) {
      c = *m_look.camera;
    } else if (m_look.mode != SkyMode::Band) {
      c = kHero;
      c.yaw += m_yaw;
      c.pitch = std::clamp(c.pitch + m_pitch, kPitchMin, kPitchMax);
      c.dist = std::clamp(c.dist * m_zoom, kDistMin, kDistMax);
    }
    if (!m_morph) return c;
    const float e = m_morph->eased;
    const auto mix = [e](float a, float b) { return a + (b - a) * e; };
    const GalaxyCamera& f = m_morph->from;
    return {.yaw = mix(f.yaw, c.yaw), .pitch = mix(f.pitch, c.pitch), .dist = mix(f.dist, c.dist), .sx = mix(f.sx, c.sx), .sy = mix(f.sy, c.sy)};
  }

  void Sky::applyCamera() {
    if (m_galaxy == nullptr) return;
    m_galaxy->setCamera(camera());
    placeLeaders();
  }

  std::unique_ptr<Node> Sky::build(const SkyLook& look, float width, float height, float top, std::unique_ptr<Node> front,
                                   std::vector<SkyAnchor> anchors) {
    const std::optional<SkyMode> before = m_shown;
    const GalaxyCamera shownCamera = camera();
    const float shownHeight = m_morph ? m_morph->fromHeight + (m_height - m_morph->fromHeight) * m_morph->eased : m_height;
    m_look = look;
    m_shown = look.mode;
    m_height = height;
    m_follower = nullptr;
    m_scale = m_window.uiScale();
    const float s = m_scale;
    const Shape shape{.cool = look.cool, .forming = look.forming};
    startPoints(shape);
    auto layer = std::make_unique<Layer>(*this);
    layer->setSize(width, height);
    m_layer = layer.get();

    auto galaxy = std::make_unique<GalaxyNode>();
    galaxy->setCacheOwner(this);
    galaxy->setSize(width, height);
    // A galaxy of another shape stays off until this one is built.
    galaxy->setPoints(m_pointsShape == shape ? m_points : nullptr);
    galaxy->setDim(look.dim);
    galaxy->setLightSky(look.light ? std::optional(GalaxyLightSky{}) : std::nullopt);
    galaxy->setCamera(camera());
    galaxy->setLife({.angle = m_angle, .glow = 1.0F});
    m_galaxy = static_cast<GalaxyNode*>(layer->addChild(std::move(galaxy)));

    if (look.mode == SkyMode::Hero) {
      auto area = ui::inputArea({.width = width, .height = std::max(0.0F, height - top)});
      area->setPosition(0.0F, top);
      area->setOnPress([this](const InputArea::PointerData& data) {
        m_dragging = data.pressed;
        m_dragAt = {data.sceneX, data.sceneY};
        if (m_galaxy != nullptr) m_galaxy->setMoving(m_dragging || m_morph.has_value());
      });
      area->setOnMotion([this](const InputArea::PointerData& data) {
        if (!m_dragging) return;
        const float dx = (data.sceneX - m_dragAt[0]) / m_scale;
        const float dy = (data.sceneY - m_dragAt[1]) / m_scale;
        m_dragAt = {data.sceneX, data.sceneY};
        m_yaw -= dx * kYawPerPx;
        m_pitch = std::clamp(kHero.pitch + m_pitch + dy * kPitchPerPx, kPitchMin, kPitchMax) - kHero.pitch;
        applyCamera();
      });
      area->setOnCancel([this] {
        m_dragging = false;
        if (m_galaxy != nullptr) m_galaxy->setMoving(m_morph.has_value());
      });
      area->setOnAxisHandler([this](const InputArea::PointerData& data) {
        const float steps = data.scrollSteps();
        if (steps == 0.0F) return true;
        m_zoom = std::clamp(m_zoom * std::exp(steps * kZoomStep), kDistMin / kHero.dist, kDistMax / kHero.dist);
        if (m_galaxy != nullptr) m_galaxy->setMoving(true);
        applyCamera();
        AnimationManager& animations = m_window.animations();
        animations.cancelForOwner(&m_zoom);
        animations.animateTimer(0.0F, 1.0F, kWheelRestMs, Easing::Linear, [](float) {}, [this] {
          if (m_galaxy != nullptr && !m_dragging && !m_morph) m_galaxy->setMoving(false);
        }, &m_zoom);
        return true;
      });
      layer->addChild(std::move(area));
    }

    m_anchors = std::move(anchors);
    m_leaders.clear();
    const Color core = rgbaHex(look.light ? Tokens::componentLinkCoreLightSky : Tokens::componentLinkCore);
    for (std::size_t i = 0; i < m_anchors.size(); ++i) {
      Leader leader;
      Color c = core;
      c.a = Tokens::componentLinkLeaderAlpha;
      leader.line = static_cast<Box*>(layer->addChild(ui::box({.fill = fixedColorSpec(c), .height = s})));
      leader.halo = static_cast<Box*>(layer->addChild(ui::box({.fill = fixedColorSpec(core)})));
      leader.dot = static_cast<Box*>(layer->addChild(ui::box({.fill = fixedColorSpec(core)})));
      m_leaders.push_back(leader);
    }

    if (front != nullptr) {
      front->setSize(width, height);
      front->setPosition(0.0F, 0.0F);
      layer->front = layer->addChild(std::move(front));
    }
    layer->buttons = layer->addChild(m_window.buildWindowButtons());
    if (m_life != SkyLife::Still && m_loop == 0) startLoop();
    const auto morphs = [](SkyMode mode) { return mode == SkyMode::Hero || mode == SkyMode::Band; };
    if (before && *before != look.mode && morphs(*before) && morphs(look.mode) && motionAllowed()) {
      m_morph = Morph{.from = shownCamera, .fromHeight = shownHeight};
      startMorph();
    } else if (before != look.mode || !morphs(look.mode)) {
      endMorph();
    }
    syncArrivals(before.has_value() && motionAllowed());
    applyMorph();
    return layer;
  }

  void Sky::setFollower(Node* node) {
    m_follower = node;
    m_followerY = node != nullptr ? node->translationY() : 0.0F;
    applyMorph();
  }

  void Sky::hide() {
    m_shown.reset();
    endMorph();
  }

  void Sky::startMorph() {
    AnimationManager& animations = m_window.animations();
    animations.cancelForOwner(&m_morph);
    m_morph->id = animations.animateTimer(
        0.0F, 1.0F, static_cast<float>(Tokens::componentLinkMorphMs), Easing::EaseOutQuad,
        [this](float eased) {
          if (!m_morph) return;
          m_morph->eased = eased;
          applyMorph();
        },
        [this] {
          m_morph.reset();
          applyMorph();
        },
        &m_morph);
  }

  void Sky::endMorph() {
    if (!m_morph) return;
    m_window.animations().cancelForOwner(&m_morph);
    m_morph.reset();
  }

  // The sky at the morph's progress: galaxy height and camera, the front fading in, the follower on the sky's bottom.
  void Sky::applyMorph() {
    if (m_layer == nullptr || m_galaxy == nullptr) return;
    const float e = m_morph ? m_morph->eased : 1.0F;
    const float height = m_morph ? m_morph->fromHeight + (m_height - m_morph->fromHeight) * e : m_height;
    m_galaxy->setFrameSize(m_layer->width(), height);
    m_galaxy->setMoving(m_morph.has_value() || m_dragging);
    m_galaxy->setCamera(camera());
    // Only Hero and Band morph: the scrim fades in toward Band and out toward Hero.
    const float scrim = m_look.mode == SkyMode::Band ? e : m_morph ? 1.0F - e : 0.0F;
    m_galaxy->setScrim(bandScrim(m_look.light, scrim, m_scale));
    if (m_layer->front != nullptr) m_layer->front->setOpacity(e);
    if (m_follower != nullptr) m_follower->setTranslation(m_follower->translationX(), m_followerY + height - m_height);
    placeLeaders();
  }

  void Sky::syncArrivals(bool fade) {
    AnimationManager& animations = m_window.animations();
    std::vector<Arrival> next;
    next.reserve(m_anchors.size());
    for (const SkyAnchor& anchor : m_anchors) {
      const auto it = std::ranges::find(m_arrivals, anchor.star, &Arrival::star);
      next.push_back(it != m_arrivals.end() ? *it : Arrival{.star = anchor.star, .t = fade ? 0.0F : 1.0F});
    }
    for (const Arrival& old : m_arrivals) {
      if (old.id != 0 && std::ranges::find(next, old.star, &Arrival::star) == next.end()) animations.cancel(old.id);
    }
    m_arrivals = std::move(next);
    for (Arrival& a : m_arrivals) {
      if (a.t >= 1.0F || a.id != 0) continue;
      const auto star = a.star;
      const auto find = [this, star] { return std::ranges::find(m_arrivals, star, &Arrival::star); };
      a.id = animations.animateTimer(
          0.0F, 1.0F, static_cast<float>(Tokens::componentLinkLeaderInMs), Easing::EaseOutQuad,
          [this, find](float t) {
            if (const auto it = find(); it != m_arrivals.end()) it->t = t;
            placeLeaders();
          },
          [this, find] {
            if (const auto it = find(); it != m_arrivals.end()) *it = {.star = it->star};
            placeLeaders();
          },
          &m_arrivals);
    }
  }

  float Sky::arrival(const std::array<float, 3>& star) const {
    const auto it = std::ranges::find(m_arrivals, star, &Arrival::star);
    return it != m_arrivals.end() ? it->t : 1.0F;
  }

  // Runs each life tick: it resizes only for paint (setFrameSize), so a tick never lays out the window.
  void Sky::placeLeaders() {
    if (m_layer == nullptr || m_galaxy == nullptr) return;
    placeRings();
    const float s = m_scale;
    const float t = m_lifeTime;
    const bool alive = m_loop != 0;
    const bool band = m_look.mode == SkyMode::Band;
    const float floor = m_galaxy->height() - kLeaderFloor * s;
    for (std::size_t i = 0; i < m_leaders.size() && i < m_anchors.size(); ++i) {
      const Leader& leader = m_leaders[i];
      const SkyAnchor& anchor = m_anchors[i];
      const float in = arrival(anchor.star);
      if (anchor.card != nullptr) anchor.card->setOpacity(in);
      const auto star = m_galaxy->project(anchor.star);
      const bool shown = anchor.card != nullptr && star && (*star)[1] <= floor;
      leader.line->setVisible(shown);
      leader.halo->setVisible(shown);
      leader.dot->setVisible(shown);
      if (!shown) continue;
      const auto [px, py] = *star;
      const auto [cx, cy0] = offsetIn(anchor.card, m_layer);
      const float cy = cy0 + std::min(anchor.card->height() / 2.0F, kLeaderReach * s);
      const float dx = cx - px;
      const float dy = cy - py;
      // While the card fades in, the line grows from the star toward it.
      const float length = std::hypot(dx, dy) * in;
      const float ex = px + dx * in;
      const float ey = py + dy * in;
      leader.line->setFrameSize(length, s);
      leader.line->setPosition((px + ex - length) / 2.0F, (py + ey - s) / 2.0F);
      leader.line->setRotation(std::atan2(dy, dx));
      leader.line->setOpacity(band ? kBandLeaderAlpha / Tokens::componentLinkLeaderAlpha : 1.0F);
      leader.dot->setOpacity(in);
      const float k = alive ? 1.0F + 0.35F * std::sin(t * 3.0F + anchor.star[0] * 7.0F) : 1.0F;
      const float halo = Tokens::componentLinkLeaderHalo * (band ? kBandHaloShare : 1.0F) * s * k;
      leader.halo->setFrameSize(2.0F * halo, 2.0F * halo);
      leader.halo->setRadius(halo);
      leader.halo->setPosition(px - halo, py - halo);
      leader.halo->setOpacity(in * (alive ? 0.22F / k : 0.2F));
      const float dot = Tokens::componentLinkLeaderDot * (band ? kBandDotShare : 1.0F) * s;
      leader.dot->setFrameSize(2.0F * dot, 2.0F * dot);
      leader.dot->setRadius(dot);
      leader.dot->setPosition(px - dot, py - dot);
    }
  }

  // A call (pages.html "rings leave the core"): 3 rings from the projected core, phased by the life clock.
  void Sky::placeRings() {
    const auto core = m_life == SkyLife::Call ? m_galaxy->project({0.0F, 0.0F, 0.0F}) : std::nullopt;
    if (!core) {
      m_galaxy->setRings({});
      return;
    }
    const float s = m_scale;
    const bool band = m_look.mode == SkyMode::Band;
    const Color call = rgbaHex(Tokens::componentLinkCall);
    const float cycle = static_cast<float>(Tokens::componentLinkCallWaveMs) / 1000.0F;
    m_galaxy->setRings({.centre = *core, .color = {call.r, call.g, call.b}, .phase = std::fmod(m_lifeTime / cycle, 1.0F),
                        .start = (band ? Tokens::componentLinkCallWaveBandStart : Tokens::componentLinkCallWaveStart) * s,
                        .end = (band ? Tokens::componentLinkCallWaveBandEnd : Tokens::componentLinkCallWaveEnd) * s,
                        .ratio = band ? Tokens::componentLinkCallWaveBandRatio : Tokens::componentLinkCallWaveRatio,
                        .width = Tokens::componentLinkCallWaveWidth * s, .alpha = Tokens::componentLinkCallWaveAlpha});
  }

  void Sky::setLife(SkyLife life) {
    if (!motionAllowed()) life = SkyLife::Still;
    if (life == m_life) return;
    m_life = life;
    if (life == SkyLife::Still) {
      stopLoop();
      if (m_galaxy != nullptr) m_galaxy->setLife({.angle = m_angle, .glow = 1.0F});
      placeLeaders();
    } else if (m_loop == 0 && m_layer != nullptr) {
      startLoop();
    }
  }

  void Sky::startLoop() {
    const float period = static_cast<float>(Tokens::componentLinkBreatheMs);
    m_loop = m_window.animations().animateTimer(
        0.0F, 1.0F, period, Easing::Linear, [this](float phase) { tick(phase); },
        [this, period] {
          m_loop = 0;
          m_loopStart += period / 1000.0F;
          if (m_life != SkyLife::Still && m_layer != nullptr) startLoop();
        },
        this);
  }

  void Sky::stopLoop() {
    if (m_loop == 0) return;
    m_window.animations().cancel(m_loop);
    m_loop = 0;
    m_loopStart = m_lifeTime;
  }

  void Sky::tick(float phase) {
    const float time = m_loopStart + phase * static_cast<float>(Tokens::componentLinkBreatheMs) / 1000.0F;
    m_angle = std::fmod(m_angle + lifeSpin(m_life) * std::max(0.0F, time - m_lifeTime), kTwoPi);
    m_lifeTime = time;
    if (m_galaxy == nullptr) return;
    const float glow = 1.0F + lifeGlow(m_life) * 0.5F * (1.0F - std::cos(kTwoPi * phase));
    m_galaxy->setLife({.angle = m_angle, .glow = glow});
    placeLeaders();
  }

} // namespace linkapp
