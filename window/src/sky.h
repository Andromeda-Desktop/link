#pragma once

#include "render/galaxy/galaxy_model.h"
#include "render/galaxy/galaxy_view.h"
#include "render/scene/node.h"

#include <array>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>

class AppWindow;
class Box;
class GalaxyNode;

namespace linkapp {

  // Hero: Home, the sky over the sheet. Band: every other page, a low strip. Full: the whole column (Mirror, Pair,
  // Home with no phone).
  enum class SkyMode : std::uint8_t { Hero, Band, Full };
  // What runs on the phone: music turns the galaxy and breathes its core; a stream, transfer or call turns it slowly;
  // Still draws no frames.
  enum class SkyLife : std::uint8_t { Still, Busy, Music };

  struct SkyLook {
    SkyMode mode = SkyMode::Hero;
    float dim = 1.0F;     // the galaxy's own light, 0..1
    bool light = false;   // the Aurora sky of the light theme
    bool cool = false;    // battery under 15 %: the core glows red
    bool forming = false; // Pair: a young galaxy, few stars and knots
    std::optional<GalaxyCamera> camera; // a Full page's fixed camera; none: the Hero camera with the user's turn
    bool operator==(const SkyLook&) const = default;
  };

  // A running card on the sky and the galaxy point it belongs to.
  struct SkyAnchor {
    Node* card = nullptr;
    std::array<float, 3> star{};
  };

  // The window's sky: the phone's galaxy, drag to turn and wheel to zoom (Hero only), the leaders from each anchor's
  // star to its card, and the life loop. One per window; its nodes belong to the last build.
  class Sky {
  public:
    explicit Sky(AppWindow& window);
    ~Sky();
    Sky(const Sky&) = delete;
    Sky& operator=(const Sky&) = delete;

    // width x height in build px. `front` lies over the galaxy (identity, cards); `top` is the height that moves
    // the window, where the galaxy takes no drag. Anchors point into `front`.
    [[nodiscard]] std::unique_ptr<Node> build(const SkyLook& look, float width, float height, float top, std::unique_ptr<Node> front,
                                              std::vector<SkyAnchor> anchors);
    // The node under the sky (the sheet): while the sky morphs between Hero and Band it moves with the sky's bottom.
    void setFollower(Node* node);
    // This build draws no sky: the next build starts without a morph or card fade-in.
    void hide();
    void setLife(SkyLife life);

  private:
    class Layer;
    // The galaxy the points are built for.
    struct Shape {
      bool cool = false;
      bool forming = false;
      bool operator==(const Shape&) const = default;
    };
    void startPoints(Shape shape);
    void startLoop();
    void stopLoop();
    void tick(float phase);
    [[nodiscard]] GalaxyCamera camera() const;
    void applyCamera();
    void placeLeaders();
    void startMorph();
    void endMorph();
    void applyMorph();
    void syncArrivals(bool fade);
    [[nodiscard]] float arrival(const std::array<float, 3>& star) const;

    AppWindow& m_window;
    std::shared_ptr<int> m_alive = std::make_shared<int>(0);
    std::shared_ptr<const GalaxyPoints> m_points;
    Shape m_pointsShape;
    bool m_building = false;
    SkyLook m_look;
    // The user's turn and zoom of the Hero camera; the Band and Full cameras stay fixed.
    float m_yaw = 0.0F;
    float m_pitch = 0.0F;
    float m_zoom = 1.0F;
    std::array<float, 2> m_dragAt{};
    bool m_dragging = false;
    SkyLife m_life = SkyLife::Still;
    std::uint32_t m_loop = 0;     // AnimationManager id while the life loop runs
    float m_loopStart = 0.0F;     // seconds of life before the current loop cycle
    float m_lifeTime = 0.0F;      // seconds the galaxy has lived
    float m_angle = 0.0F;         // the life turn so far, rad
    Layer* m_layer = nullptr;     // of the last build
    GalaxyNode* m_galaxy = nullptr;
    std::vector<SkyAnchor> m_anchors;
    struct Leader {
      Box* line = nullptr;
      Box* halo = nullptr;
      Box* dot = nullptr;
    };
    std::vector<Leader> m_leaders;
    float m_scale = 1.0F;
    // The mode of the last build that drew the sky; none after hide().
    std::optional<SkyMode> m_shown;
    // Hero <-> Band: the camera and height tween from where the sky was. `eased` runs 0..1.
    struct Morph {
      GalaxyCamera from;
      float fromHeight = 0.0F;
      float eased = 0.0F;
      std::uint32_t id = 0;
    };
    std::optional<Morph> m_morph;
    Node* m_follower = nullptr;
    float m_followerY = 0.0F;     // the follower's own translation
    float m_height = 0.0F;        // the sky's height at rest (this build)
    // A card that appears fades in and its leader draws from the star; `t` runs 0..1, 1 = shown.
    struct Arrival {
      std::array<float, 3> star{};
      float t = 1.0F;
      std::uint32_t id = 0;
    };
    std::vector<Arrival> m_arrivals;
  };

} // namespace linkapp
