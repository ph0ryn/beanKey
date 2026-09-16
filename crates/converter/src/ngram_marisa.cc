#include <marisa/trie.h>

#include <cstddef>
#include <cstdint>
#include <new>

struct BeanKeyMarisaTrie {
  marisa::Trie trie;
};

extern "C" BeanKeyMarisaTrie *beankey_marisa_load(const char *path) noexcept {
  if (path == nullptr) {
    return nullptr;
  }
  try {
    auto *trie = new BeanKeyMarisaTrie;
    trie->trie.load(path);
    return trie;
  } catch (...) {
    return nullptr;
  }
}

extern "C" void beankey_marisa_free(BeanKeyMarisaTrie *trie) noexcept {
  delete trie;
}

using BeanKeyMarisaVisitor = bool (*)(const std::uint8_t *, std::size_t,
                                      void *);

extern "C" bool beankey_marisa_predictive_search(
    const BeanKeyMarisaTrie *trie, const std::uint8_t *query,
    std::size_t query_size, BeanKeyMarisaVisitor visitor,
    void *visitor_context) noexcept {
  if (trie == nullptr || (query == nullptr && query_size != 0) ||
      visitor == nullptr) {
    return false;
  }
  try {
    marisa::Agent agent;
    agent.set_query(reinterpret_cast<const char *>(query), query_size);
    while (trie->trie.predictive_search(agent)) {
      const auto &key = agent.key();
      if (!visitor(reinterpret_cast<const std::uint8_t *>(key.ptr()),
                   key.length(), visitor_context)) {
        return false;
      }
    }
    return true;
  } catch (...) {
    return false;
  }
}
