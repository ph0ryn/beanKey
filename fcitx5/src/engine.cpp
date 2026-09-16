#include "engine.h"

#include "key_event.h"

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx-utils/misc.h>
#include <fcitx-utils/textformatflags.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/statusarea.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterfacemanager.h>

#include <algorithm>
#include <atomic>
#include <cstdlib>
#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace fcitx {
namespace {

constexpr std::uint32_t kProtocolVersion = 1;
constexpr std::uint32_t kCandidatePageSize = 9;
constexpr auto kStartupTimeout = std::chrono::milliseconds(5000);

std::atomic<std::uint64_t> nextSessionId{1};

std::string socketPath() {
  const char *runtimeDirectory = std::getenv("XDG_RUNTIME_DIR");
  if (runtimeDirectory == nullptr || runtimeDirectory[0] != '/') {
    return {};
  }
  return std::string(runtimeDirectory) + "/beankey/daemon.sock";
}

std::size_t characterCount(const std::string &text) {
  return text.empty() ? 0 : utf8::length(text);
}

int byteOffset(const std::string &text, std::size_t characterOffset) {
  if (text.empty() || characterOffset == 0) {
    return 0;
  }
  const auto clamped = std::min(characterOffset, characterCount(text));
  return static_cast<int>(utf8::ncharByteLength(text.begin(), clamped));
}

std::unique_ptr<CommonCandidateList> makeCandidateList() {
  auto candidates = std::make_unique<CommonCandidateList>();
  candidates->setPageSize(kCandidatePageSize);
  candidates->setLayoutHint(CandidateLayoutHint::Vertical);
  return candidates;
}

class BeanKeyCandidateWord final : public CandidateWord {
public:
  BeanKeyCandidateWord(std::uint32_t index, std::string text,
                       std::string annotation, bool regularCandidate,
                       BeanKeyState *state)
      : CandidateWord(Text(std::move(text))), index_(index),
        regularCandidate_(regularCandidate), state_(state) {
    if (!annotation.empty()) {
      setComment(Text(std::move(annotation)));
    }
  }

  void select(InputContext *) const override {
    state_->selectCandidate(index_);
  }

  std::uint32_t index() const { return index_; }
  bool regularCandidate() const { return regularCandidate_; }
  void forget() const { state_->forgetCandidate(index_); }
  void requestTypoCorrections() const { state_->requestTypoCorrections(); }

private:
  std::uint32_t index_;
  bool regularCandidate_;
  BeanKeyState *state_;
};

class BeanKeyTypoCorrectionWord final : public CandidateWord {
public:
  BeanKeyTypoCorrectionWord(std::uint32_t index, std::string correctedInput,
                            std::string convertedText, BeanKeyState *state)
      : CandidateWord(Text(std::move(convertedText))), index_(index),
        state_(state) {
    setComment(Text(std::move(correctedInput)));
  }

  void select(InputContext *) const override {
    state_->selectTypoCorrection(index_);
  }

  std::uint32_t index() const { return index_; }

private:
  std::uint32_t index_;
  BeanKeyState *state_;
};

class BeanKeyCandidateActions final : public ActionableCandidateList {
public:
  BeanKeyCandidateActions(bool learningWritable, bool lmTypoAvailable)
      : learningWritable_(learningWritable), lmTypoAvailable_(lmTypoAvailable) {
  }

  bool hasAction(const CandidateWord &candidate) const override {
    const auto *word = dynamic_cast<const BeanKeyCandidateWord *>(&candidate);
    return word != nullptr && word->regularCandidate() &&
           (learningWritable_ || lmTypoAvailable_);
  }

  std::vector<CandidateAction>
  candidateActions(const CandidateWord &candidate) const override {
    if (!hasAction(candidate)) {
      return {};
    }
    std::vector<CandidateAction> actions;
    if (learningWritable_) {
      CandidateAction forget;
      forget.setId(kForgetAction);
      forget.setText("Forget");
      forget.setIcon("edit-delete");
      actions.push_back(std::move(forget));
    }
    if (lmTypoAvailable_) {
      CandidateAction typo;
      typo.setId(kTypoCorrectionAction);
      typo.setText("Correct typos");
      typo.setIcon("tools-check-spelling");
      actions.push_back(std::move(typo));
    }
    return actions;
  }

  void triggerAction(const CandidateWord &candidate, int id) override {
    const auto *word = dynamic_cast<const BeanKeyCandidateWord *>(&candidate);
    if (word != nullptr && word->regularCandidate() && id == kForgetAction &&
        learningWritable_) {
      word->forget();
    } else if (word != nullptr && word->regularCandidate() &&
               id == kTypoCorrectionAction && lmTypoAvailable_) {
      word->requestTypoCorrections();
    }
  }

private:
  static constexpr int kForgetAction = 0;
  static constexpr int kTypoCorrectionAction = 1;
  bool learningWritable_;
  bool lmTypoAvailable_;
};

} // namespace

BeanKeyState::BeanKeyState(InputContext *inputContext, BeanKeyEngine *engine)
    : inputContext_(inputContext), engine_(engine) {}

BeanKeyState::~BeanKeyState() = default;

bool BeanKeyState::focusIn() { return start(); }

void BeanKeyState::focusOut(bool commitComposition) {
  if (commitComposition && started_) {
    static_cast<void>(this->commitComposition());
  }
  if (started_ && engine_->client().connected()) {
    auto request = envelope();
    request.mutable_end_session();
    static_cast<void>(
        engine_->client().request(request, engine_->requestTimeout()));
  }
  started_ = false;
  sessionId_.clear();
  clearUi();
}

bool BeanKeyState::processKey(KeyEvent &event) {
  if (event.isRelease()) {
    return false;
  }
  if (!start()) {
    failSession();
    return false;
  }

  const auto candidateList = inputContext_->inputPanel().candidateList();
  const int selection = event.key().digitSelection();
  if (candidateList && selection >= 0 && selection < candidateList->size()) {
    const auto &candidate = candidateList->candidate(selection);
    if (const auto *word =
            dynamic_cast<const BeanKeyTypoCorrectionWord *>(&candidate)) {
      return selectTypoCorrection(word->index());
    }
    if (selectedCandidate_ >= 0) {
      if (const auto *word =
              dynamic_cast<const BeanKeyCandidateWord *>(&candidate)) {
        return selectCandidate(word->index());
      }
    }
  }

  auto request = envelope();
  auto *key = request.mutable_key_event();
  beankey::populateKeyEvent(event, key);
  if (key->action() == beankey::v1::USER_ACTION_UNSPECIFIED) {
    return false;
  }
  fillSurroundingText(key->mutable_surrounding_text());

  std::vector<beankey::v1::CursorAction> actions;
  if (selectedCandidate_ >= 0 &&
      static_cast<std::size_t>(selectedCandidate_) < candidateActions_.size()) {
    actions = candidateActions_[selectedCandidate_];
  }
  return send(std::move(request), actions);
}

bool BeanKeyState::selectCandidate(std::uint32_t index) {
  if (!start() || index >= candidateActions_.size()) {
    failSession();
    return false;
  }
  auto request = envelope();
  request.mutable_select_candidate()->set_index(index);
  return send(std::move(request), candidateActions_[index]);
}

bool BeanKeyState::forgetCandidate(std::uint32_t index) {
  if (!start() || index >= candidateActions_.size()) {
    failSession();
    return false;
  }
  auto request = envelope();
  request.mutable_forget_candidate()->set_index(index);
  return send(std::move(request));
}

bool BeanKeyState::selectTypoCorrection(std::uint32_t index) {
  if (!start()) {
    failSession();
    return false;
  }
  auto request = envelope();
  request.mutable_select_typo_correction()->set_index(index);
  return send(std::move(request));
}

bool BeanKeyState::resetLearning() {
  if (!start()) {
    failSession();
    return false;
  }
  auto request = envelope();
  request.mutable_reset_learning();
  return send(std::move(request));
}

bool BeanKeyState::requestTypoCorrections() {
  if (!start()) {
    failSession();
    return false;
  }
  auto request = envelope();
  request.mutable_request_typo_corrections();
  const auto response =
      engine_->client().request(request, engine_->requestTimeout());
  if (!response || response->protocol_version() != kProtocolVersion ||
      response->request_id() != request.request_id() ||
      response->session_id() != sessionId_ ||
      !response->has_typo_correction_response()) {
    failSession();
    return false;
  }
  showTypoCorrections(response->typo_correction_response());
  return true;
}

bool BeanKeyState::learningAvailable() const { return learningAvailable_; }

bool BeanKeyState::learningWritable() const { return learningWritable_; }

void BeanKeyState::reset() {
  if (!started_ || !engine_->client().connected()) {
    clearUi();
    started_ = false;
    return;
  }
  auto request = envelope();
  request.mutable_reset_session();
  if (!send(std::move(request))) {
    failSession();
  }
}

bool BeanKeyState::start() {
  if (started_) {
    return true;
  }
  if (!engine_->ensureConnected()) {
    return false;
  }
  sessionId_ = std::to_string(nextSessionId.fetch_add(1));
  nextRequestId_ = 1;
  auto request = envelope();
  auto *start = request.mutable_start_session();
  start->set_input_style(beankey::v1::INPUT_STYLE_UNSPECIFIED);
  start->set_keyboard_language(beankey::v1::KEYBOARD_LANGUAGE_UNSPECIFIED);
  fillSurroundingText(start->mutable_surrounding_text());
  const auto response =
      engine_->client().request(request, engine_->requestTimeout());
  if (!response || response->protocol_version() != kProtocolVersion ||
      response->request_id() != request.request_id() ||
      response->session_id() != sessionId_ || !response->has_state_response()) {
    engine_->client().disconnect();
    sessionId_.clear();
    return false;
  }
  started_ = true;
  return apply(*response, {});
}

bool BeanKeyState::pageCandidates(
    beankey::v1::PageCandidates::Direction direction) {
  if (candidateActions_.empty()) {
    return false;
  }
  auto request = envelope();
  auto *page = request.mutable_page_candidates();
  page->set_direction(direction);
  page->set_page_size(kCandidatePageSize);
  return send(std::move(request));
}

bool BeanKeyState::commitComposition() {
  auto request = envelope();
  request.mutable_commit_composition();
  return send(std::move(request));
}

bool BeanKeyState::send(
    beankey::v1::Envelope request,
    const std::vector<beankey::v1::CursorAction> &commitActions) {
  const auto response =
      engine_->client().request(request, engine_->requestTimeout());
  if (!response || response->protocol_version() != kProtocolVersion ||
      response->request_id() != request.request_id() ||
      response->session_id() != sessionId_ || !response->has_state_response()) {
    failSession();
    return false;
  }
  return apply(*response, commitActions);
}

bool BeanKeyState::apply(
    const beankey::v1::Envelope &response,
    const std::vector<beankey::v1::CursorAction> &commitActions) {
  const auto &state = response.state_response();
  lmTypoAvailable_ = state.lm_typo_available();
  learningAvailable_ = state.learning_available();
  learningWritable_ = state.learning_writable();
  if (!state.commit().empty()) {
    auto cursor = static_cast<std::int64_t>(characterCount(state.commit()));
    for (const auto &action : commitActions) {
      cursor += action.move();
    }
    cursor =
        std::clamp<std::int64_t>(cursor, 0, characterCount(state.commit()));
    if (inputContext_->capabilityFlags().test(
            CapabilityFlag::CommitStringWithCursor)) {
      inputContext_->commitStringWithCursor(state.commit(),
                                            static_cast<std::size_t>(cursor));
    } else {
      inputContext_->commitString(state.commit());
    }
  }

  candidateActions_.clear();
  auto candidates = makeCandidateList();
  const bool selecting =
      state.candidate_window() == beankey::v1::CANDIDATE_WINDOW_SELECTING;
  if (selecting) {
    candidates->setSelectionKey(Key::keyListFromString("1 2 3 4 5 6 7 8 9"));
  } else {
    candidateWindowStart_ = 0;
  }
  candidates->setActionableImpl(std::make_unique<BeanKeyCandidateActions>(
      learningWritable_, lmTypoAvailable_));
  selectedCandidate_ = state.selected_candidate();
  if (selecting && selectedCandidate_ >= 0) {
    const auto windowEnd =
        candidateWindowStart_ + static_cast<int>(kCandidatePageSize);
    if (selectedCandidate_ < candidateWindowStart_) {
      candidateWindowStart_ = selectedCandidate_;
    } else if (selectedCandidate_ >= windowEnd) {
      candidateWindowStart_ =
          selectedCandidate_ - static_cast<int>(kCandidatePageSize) + 1;
    }
  }
  const auto candidateWindowEnd =
      candidateWindowStart_ + static_cast<int>(kCandidatePageSize);
  for (int index = 0; index < state.candidates_size(); ++index) {
    const auto &candidate = state.candidates(index);
    std::vector<beankey::v1::CursorAction> actions;
    actions.reserve(candidate.actions_size());
    for (const auto &action : candidate.actions()) {
      actions.push_back(action);
    }
    const auto sourceIndex = static_cast<std::size_t>(candidate.index());
    if (candidateActions_.size() <= sourceIndex) {
      candidateActions_.resize(sourceIndex + 1);
    }
    candidateActions_[sourceIndex] = std::move(actions);
    if ((selecting && index >= candidateWindowStart_ &&
         index < candidateWindowEnd) ||
        (!selecting && index == 0)) {
      candidates->append<BeanKeyCandidateWord>(
          candidate.index(), candidate.text(), candidate.annotation(),
          candidate.has_composing_count(), this);
    }
  }
  if (selectedCandidate_ >= 0 && selectedCandidate_ < state.candidates_size()) {
    candidates->setGlobalCursorIndex(selectedCandidate_ -
                                     candidateWindowStart_);
  }

  auto &panel = inputContext_->inputPanel();
  if (state.preedit().empty()) {
    panel.setPreedit(Text());
    panel.setClientPreedit(Text());
  } else {
    Text preedit;
    const auto highlighted = std::min<std::size_t>(
        state.highlighted_preedit_length(), characterCount(state.preedit()));
    const auto highlightedBytes = byteOffset(state.preedit(), highlighted);
    if (highlightedBytes > 0) {
      preedit.append(state.preedit().substr(0, highlightedBytes),
                     TextFormatFlag::HighLight);
    }
    if (static_cast<std::size_t>(highlightedBytes) < state.preedit().size()) {
      preedit.append(state.preedit().substr(highlightedBytes),
                     TextFormatFlag::Underline);
    }
    preedit.setCursor(byteOffset(state.preedit(), state.preedit_cursor()));
    panel.setPreedit(preedit);
    panel.setClientPreedit(preedit);
  }
  if (state.candidate_window() == beankey::v1::CANDIDATE_WINDOW_HIDDEN ||
      candidates->size() == 0) {
    panel.setCandidateList(nullptr);
  } else {
    panel.setCandidateList(std::move(candidates));
  }
  if (state.has_prediction()) {
    panel.setAuxDown(Text("→ " + state.prediction().display_text()));
  } else {
    panel.setAuxDown(Text());
  }
  if (state.reset() && state.preedit().empty() &&
      state.candidates_size() == 0) {
    panel.reset();
  }
  inputContext_->updatePreedit();
  inputContext_->updateUserInterface(UserInterfaceComponent::InputPanel);
  return state.consumed();
}

void BeanKeyState::showTypoCorrections(
    const beankey::v1::TypoCorrectionResponse &response) {
  if (response.candidates().empty()) {
    return;
  }
  candidateActions_.clear();
  selectedCandidate_ = -1;
  auto candidates = makeCandidateList();
  candidates->setSelectionKey(Key::keyListFromString("1 2 3 4 5 6 7 8 9"));
  for (int index = 0; index < response.candidates_size(); ++index) {
    const auto &candidate = response.candidates(index);
    candidates->append<BeanKeyTypoCorrectionWord>(
        static_cast<std::uint32_t>(index), candidate.corrected_input(),
        candidate.converted_text(), this);
  }
  candidates->setGlobalCursorIndex(0);
  inputContext_->inputPanel().setCandidateList(std::move(candidates));
  inputContext_->updateUserInterface(UserInterfaceComponent::InputPanel);
}

void BeanKeyState::fillSurroundingText(
    beankey::v1::SurroundingText *surrounding) const {
  const auto &source = inputContext_->surroundingText();
  surrounding->set_available(source.isValid());
  if (source.isValid()) {
    surrounding->set_text(source.text());
    surrounding->set_cursor(source.cursor());
    surrounding->set_anchor(source.anchor());
  }
}

void BeanKeyState::clearUi() {
  candidateActions_.clear();
  selectedCandidate_ = -1;
  candidateWindowStart_ = 0;
  lmTypoAvailable_ = false;
  learningAvailable_ = false;
  learningWritable_ = false;
  inputContext_->inputPanel().reset();
  inputContext_->updatePreedit();
  inputContext_->updateUserInterface(UserInterfaceComponent::InputPanel);
}

void BeanKeyState::failSession() {
  engine_->client().disconnect();
  started_ = false;
  sessionId_.clear();
  clearUi();
}

beankey::v1::Envelope BeanKeyState::envelope() {
  beankey::v1::Envelope request;
  request.set_protocol_version(kProtocolVersion);
  request.set_request_id(nextRequestId_++);
  request.set_session_id(sessionId_);
  return request;
}

BeanKeyEngine::BeanKeyEngine(Instance *instance)
    : instance_(instance), client_(socketPath()),
      factory_([this](InputContext &inputContext) {
        return new BeanKeyState(&inputContext, this);
      }) {
  instance_->inputContextManager().registerProperty("beanKeyState", &factory_);
  resetLearningAction_.setIcon("edit-clear-history");
  resetLearningAction_.setShortText("Reset learning");
  resetLearningAction_.setLongText("Clear all beanKey learning data");
  resetLearningAction_.connect<SimpleAction::Activated>(
      [this](InputContext *inputContext) {
        state(inputContext)->resetLearning();
      });
  resetLearningAction_.registerAction("beankey-reset-learning",
                                      &instance_->userInterfaceManager());
}

void BeanKeyEngine::activate(const InputMethodEntry &,
                             InputContextEvent &event) {
  auto *beanKeyState = state(event.inputContext());
  beanKeyState->focusIn();
  if (beanKeyState->learningAvailable()) {
    event.inputContext()->statusArea().addAction(StatusGroup::InputMethod,
                                                 &resetLearningAction_);
  }
}

void BeanKeyEngine::deactivate(const InputMethodEntry &,
                               InputContextEvent &event) {
  state(event.inputContext())
      ->focusOut(event.type() == EventType::InputContextSwitchInputMethod);
}

void BeanKeyEngine::keyEvent(const InputMethodEntry &, KeyEvent &event) {
  if (state(event.inputContext())->processKey(event)) {
    event.filterAndAccept();
  }
}

void BeanKeyEngine::reset(const InputMethodEntry &, InputContextEvent &event) {
  state(event.inputContext())->reset();
}

BeanKeyState *BeanKeyEngine::state(InputContext *inputContext) {
  return inputContext->propertyFor(&factory_);
}

beankey::Client &BeanKeyEngine::client() { return client_; }

bool BeanKeyEngine::ensureConnected() {
  return client_.ensureConnected(
      [] {
        startProcess({BEANKEY_DAEMON_PATH, "--config", BEANKEY_CONFIG_PATH});
      },
      kStartupTimeout);
}

std::chrono::milliseconds BeanKeyEngine::requestTimeout() const {
  return std::chrono::milliseconds(BEANKEY_REQUEST_TIMEOUT_MS);
}

} // namespace fcitx
