#include "engine.h"

#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>

namespace fcitx {

class BeanKeyEngineFactory final : public AddonFactory {
public:
  AddonInstance *create(AddonManager *manager) override {
    return new BeanKeyEngine(manager->instance());
  }
};

} // namespace fcitx

#ifdef FCITX_ADDON_FACTORY_V2
FCITX_ADDON_FACTORY_V2(beankey, fcitx::BeanKeyEngineFactory)
#else
FCITX_ADDON_FACTORY(fcitx::BeanKeyEngineFactory)
#endif
