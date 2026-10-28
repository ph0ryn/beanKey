#include "engine.h"
#include "fixtures.h"

#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/statusarea.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include <utility>

#include "beankey.pb.h"

namespace {

using beankey::test::readFrame;
using beankey::test::writeFrame;
using beankey::test::TestInputContext;

bool report(bool condition, const char *message) {
  if (!condition) {
    std::cerr << message << '\n';
  }
  return condition;
}

} // namespace

int main() {
  std::array<char, 64> directoryTemplate{};
  std::strcpy(directoryTemplate.data(), "/tmp/beankey-engine-test.XXXXXX");
  const char *runtimeDirectory = mkdtemp(directoryTemplate.data());
  if (!report(runtimeDirectory != nullptr,
              "failed to create runtime directory")) {
    return 1;
  }
  const std::string beanKeyDirectory =
      std::string(runtimeDirectory) + "/beankey";
  if (!report(mkdir(beanKeyDirectory.c_str(), 0700) == 0,
              "failed to create beankey runtime directory")) {
    return 1;
  }
  const std::string socketPath = beanKeyDirectory + "/daemon.sock";
  if (!report(setenv("XDG_RUNTIME_DIR", runtimeDirectory, 1) == 0,
              "failed to set runtime directory")) {
    return 1;
  }

  const int listener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  std::memcpy(address.sun_path, socketPath.c_str(), socketPath.size() + 1);
  if (!report(listener >= 0, "failed to create daemon socket") ||
      !report(bind(listener, reinterpret_cast<const sockaddr *>(&address),
                   sizeof(address)) == 0,
              "failed to bind daemon socket") ||
      !report(listen(listener, 1) == 0, "failed to listen on daemon socket")) {
    return 1;
  }

  bool serverValid = true;
  std::thread server([listener, &serverValid] {
    int connection = accept4(listener, nullptr, nullptr, SOCK_CLOEXEC);
    if (connection < 0) {
      serverValid = false;
      close(listener);
      return;
    }
    enum class ExpectedRequest {
      Start,
      Key,
      RecoveredKey,
      End,
      Forget,
      ResetLearning,
      Typo,
      SelectCandidate,
      SelectTypo,
      PredictionInput,
      PredictionTab,
      PredictionEnter,
      InvalidResponse
    };
    const auto addCandidates = [](beankey::v1::StateResponse *state) {
      for (int index = 0; index < 10; ++index) {
        auto *candidate = state->add_candidates();
        candidate->set_index(index);
        candidate->set_text(
            index == 0 ? "司会" : "Candidate " + std::to_string(index + 1));
        candidate->set_annotation(index == 0 ? "noun" : "");
        candidate->mutable_composing_count()->set_input(1);
      }
    };
    std::string selectedText = "司会";
    std::unordered_set<std::string> sessionIds;
    std::unordered_map<std::string, std::uint64_t> requestIds;
    const auto exchange = [&](ExpectedRequest expected) {
      beankey::v1::Envelope request;
      if (!request.ParseFromString(readFrame(connection))) {
        return false;
      }
      const bool expectedPayload = [&] {
        switch (expected) {
        case ExpectedRequest::Start:
          return request.has_start_session() && request.request_id() == 1 &&
                 !request.session_id().empty() &&
                 sessionIds.insert(request.session_id()).second;
        case ExpectedRequest::Key:
          return request.has_key_event();
        case ExpectedRequest::RecoveredKey:
          return request.has_key_event() && request.request_id() == 2 &&
                 request.key_event().input() == "a";
        case ExpectedRequest::End:
          return request.has_end_session();
        case ExpectedRequest::Forget:
          return request.has_forget_candidate();
        case ExpectedRequest::ResetLearning:
          return request.has_reset_learning();
        case ExpectedRequest::Typo:
          return request.has_request_typo_corrections();
        case ExpectedRequest::SelectCandidate:
          return request.has_select_candidate() &&
                 request.select_candidate().index() == 0;
        case ExpectedRequest::SelectTypo:
          return request.has_select_typo_correction();
        case ExpectedRequest::PredictionInput:
          return request.has_key_event() &&
                 request.key_event().action() ==
                     beankey::v1::USER_ACTION_INPUT &&
                 request.key_event().input() == "p";
        case ExpectedRequest::PredictionTab:
          return request.has_key_event() &&
                 request.key_event().action() == beankey::v1::USER_ACTION_TAB;
        case ExpectedRequest::PredictionEnter:
          return request.has_key_event() &&
                 request.key_event().action() == beankey::v1::USER_ACTION_ENTER;
        case ExpectedRequest::InvalidResponse:
          return request.has_key_event();
        }
        return false;
      }();
      if (expected == ExpectedRequest::Start) {
        requestIds.emplace(request.session_id(), 0);
      }
      const auto previous = requestIds.find(request.session_id());
      if (!expectedPayload || previous == requestIds.end() ||
          request.request_id() != ++previous->second) {
        shutdown(connection, SHUT_RDWR);
        return false;
      }

      beankey::v1::Envelope response;
      response.set_protocol_version(request.protocol_version());
      response.set_request_id(request.request_id());
      response.set_session_id(request.session_id());
      if (expected == ExpectedRequest::InvalidResponse) {
        response.set_request_id(request.request_id() + 1);
      }
      if (expected == ExpectedRequest::Typo) {
        auto *candidate =
            response.mutable_typo_correction_response()->add_candidates();
        candidate->set_corrected_input("かな");
        candidate->set_converted_text("仮名");
      } else {
        auto *state = response.mutable_state_response();
        state->set_lm_typo_available(true);
        state->set_learning_available(true);
        state->set_learning_writable(true);
        if (expected == ExpectedRequest::Start) {
          state->set_consumed(false);
          state->set_reset(true);
        } else if (expected == ExpectedRequest::PredictionInput ||
                   expected == ExpectedRequest::PredictionTab) {
          const bool accepting = expected == ExpectedRequest::PredictionTab;
          state->set_consumed(true);
          state->set_input_state(beankey::v1::INPUT_STATE_COMPOSING);
          state->set_preedit(accepting ? "ください" : "くだし");
          state->set_preedit_cursor(accepting ? 4 : 3);
          state->set_selected_candidate(-1);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_HIDDEN);
          if (!accepting) {
            auto *prediction = state->mutable_prediction();
            prediction->set_display_text("下さい");
            prediction->set_append_text("さい");
            prediction->set_delete_count(1);
          }
        } else if (expected == ExpectedRequest::PredictionEnter) {
          state->set_consumed(true);
          state->set_commit("ください");
          state->set_reset(true);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_HIDDEN);
        } else if ((expected == ExpectedRequest::Key ||
                    expected == ExpectedRequest::RecoveredKey) &&
                   request.key_event().action() ==
                       beankey::v1::USER_ACTION_INPUT) {
          state->set_consumed(true);
          state->set_preedit("かな");
          state->set_preedit_cursor(2);
          state->set_highlighted_preedit_length(1);
          state->set_selected_candidate(0);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_SELECTING);
          addCandidates(state);
          selectedText = request.key_event().input() == "b" ? "[]" : "司会";
          if (selectedText == "[]") {
            auto *candidate = state->mutable_candidates(0);
            candidate->set_text(selectedText);
            // Legacy field 4: one CursorAction with sint32 move = -1.
            if (!candidate->MergeFromString("\x22\x02\x08\x01")) {
              return false;
            }
          }
          state->mutable_prediction()->set_display_text("今日");
        } else if (expected == ExpectedRequest::Key &&
                   request.key_event().action() ==
                       beankey::v1::USER_ACTION_UP) {
          state->set_consumed(true);
          state->set_preedit("Candidate 9");
          state->set_preedit_cursor(11);
          state->set_highlighted_preedit_length(11);
          state->set_selected_candidate(8);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_SELECTING);
          addCandidates(state);
        } else if (expected == ExpectedRequest::Key &&
                   request.key_event().action() ==
                       beankey::v1::USER_ACTION_ENTER) {
          state->set_consumed(true);
          state->set_commit(selectedText);
          state->set_reset(true);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_HIDDEN);
        } else if (expected == ExpectedRequest::Key &&
                   request.key_event().action() ==
                       beankey::v1::USER_ACTION_TAB) {
          state->set_consumed(true);
        } else if (expected == ExpectedRequest::Forget) {
          state->set_consumed(true);
          state->set_preedit("かな");
          state->set_preedit_cursor(2);
          state->set_selected_candidate(0);
          auto *candidate = state->add_candidates();
          candidate->set_text("司会");
          candidate->mutable_composing_count()->set_input(1);
        } else if (expected == ExpectedRequest::SelectTypo) {
          state->set_consumed(true);
          state->set_commit("仮名");
          state->set_reset(true);
        } else if (expected == ExpectedRequest::SelectCandidate) {
          state->set_consumed(true);
          state->set_commit(selectedText);
          state->set_reset(true);
          state->set_candidate_window(beankey::v1::CANDIDATE_WINDOW_HIDDEN);
        } else if (expected != ExpectedRequest::Key) {
          state->set_consumed(true);
        }
      }

      std::string payload;
      return response.SerializeToString(&payload) &&
             writeFrame(connection, payload);
    };

    serverValid = exchange(ExpectedRequest::Start) && serverValid;
    serverValid = exchange(ExpectedRequest::Key) && serverValid;
    serverValid = exchange(ExpectedRequest::Forget) && serverValid;
    serverValid = exchange(ExpectedRequest::Typo) && serverValid;
    serverValid = exchange(ExpectedRequest::SelectTypo) && serverValid;
    serverValid = exchange(ExpectedRequest::Key) && serverValid;
    serverValid = exchange(ExpectedRequest::Key) && serverValid;
    serverValid = exchange(ExpectedRequest::SelectCandidate) && serverValid;
    serverValid = exchange(ExpectedRequest::PredictionInput) && serverValid;
    serverValid = exchange(ExpectedRequest::PredictionTab) && serverValid;
    serverValid = exchange(ExpectedRequest::PredictionEnter) && serverValid;
    for (int selection = 0; selection < 3; ++selection) {
      serverValid = exchange(ExpectedRequest::Key) && serverValid;
      serverValid =
          exchange(selection == 0 ? ExpectedRequest::Key
                                  : ExpectedRequest::SelectCandidate) &&
          serverValid;
    }
    serverValid = exchange(ExpectedRequest::ResetLearning) && serverValid;
    for (int context = 0; context < 3; ++context) {
      serverValid = exchange(ExpectedRequest::Start) && serverValid;
      serverValid = exchange(ExpectedRequest::Key) && serverValid;
    }
    serverValid = exchange(ExpectedRequest::InvalidResponse) && serverValid;
    close(connection);
    connection = accept4(listener, nullptr, nullptr, SOCK_CLOEXEC);
    if (connection >= 0) {
      requestIds.clear();
      serverValid = exchange(ExpectedRequest::Start) && serverValid;
      serverValid = exchange(ExpectedRequest::RecoveredKey) && serverValid;
      serverValid = exchange(ExpectedRequest::Start) && serverValid;
      serverValid = exchange(ExpectedRequest::RecoveredKey) && serverValid;
      serverValid = exchange(ExpectedRequest::Key) && serverValid;
      serverValid = exchange(ExpectedRequest::Key) && serverValid;
      serverValid = exchange(ExpectedRequest::End) && serverValid;
      serverValid = exchange(ExpectedRequest::End) && serverValid;
      close(connection);
    } else {
      serverValid = false;
    }
    close(listener);
  });

  bool valid = true;
  {
    char argument0[] = "beankey-engine-test";
    char argument1[] = "--disable=all";
    char *arguments[] = {argument0, argument1};
    fcitx::Instance instance(2, arguments);
    valid =
        report(instance.initialized(), "Fcitx instance did not initialize") &&
        valid;
    valid = report(unsetenv("XDG_STATE_HOME") == 0 && unsetenv("HOME") == 0,
                   "failed to clear state directory variables") &&
            valid;
    fcitx::BeanKeyEngine engine(&instance);
    TestInputContext inputContext(instance.inputContextManager());
    inputContext.setCapabilityFlags(fcitx::CapabilityFlag::Preedit);
    fcitx::InputMethodEntry entry("beankey", "beanKey", "ja", "beankey");

    fcitx::FocusInEvent activateEvent(&inputContext);
    engine.activate(entry, activateEvent);
    const auto statusActions =
        inputContext.statusArea().actions(fcitx::StatusGroup::InputMethod);
    valid = report(statusActions.size() == 1,
                   "learning reset is not exposed in the Fcitx status area") &&
            valid;
    if (!statusActions.empty()) {
      valid = report(statusActions.front()->shortText(&inputContext) ==
                         "Reset learning",
                     "Fcitx learning reset action has the wrong label") &&
              valid;
    }

    fcitx::KeyEvent printable(&inputContext, fcitx::Key(FcitxKey_a));
    engine.keyEvent(entry, printable);
    valid = report(printable.accepted(),
                   "consumed printable key was not accepted") &&
            valid;
    valid =
        report(inputContext.inputPanel().clientPreedit().toString() == "かな",
               "daemon preedit did not reach Fcitx") &&
        valid;
    valid = report(inputContext.inputPanel().auxDown().toString() == "→ 今日",
                   "prediction did not reach the Fcitx auxiliary UI") &&
            valid;
    const auto candidates = inputContext.inputPanel().candidateList();
    valid = report(candidates && candidates->size() == 9,
                   "daemon candidates did not reach Fcitx") &&
            valid;
    valid = report(candidates && candidates->layoutHint() ==
                                     fcitx::CandidateLayoutHint::Vertical,
                   "daemon candidates did not request a vertical layout") &&
            valid;
    if (candidates && candidates->size() == 9) {
      valid = report(candidates->candidate(0).comment().toString() == "noun",
                     "candidate annotation did not reach Fcitx") &&
              valid;
    }
    for (const auto &shortcut : {
             fcitx::Key(FcitxKey_a, fcitx::KeyState::Super),
             fcitx::Key(FcitxKey_period, fcitx::KeyState::Super),
             fcitx::Key(FcitxKey_1, fcitx::KeyState::Super),
             fcitx::Key(FcitxKey_1, fcitx::KeyState::Meta),
             fcitx::Key(FcitxKey_1, fcitx::KeyState::Hyper),
             fcitx::Key(FcitxKey_j,
                        {fcitx::KeyState::Ctrl, fcitx::KeyState::Super})}) {
      fcitx::KeyEvent shortcutEvent(&inputContext, shortcut);
      engine.keyEvent(entry, shortcutEvent);
      valid = report(!shortcutEvent.accepted() &&
                         inputContext.inputPanel().clientPreedit().toString() ==
                             "かな" &&
                         inputContext.inputPanel().candidateList() == candidates &&
                         inputContext.committed().empty(),
                     "system shortcut changed the IME session or candidate UI") &&
              valid;
    }
    auto *actionable = candidates ? candidates->toActionable() : nullptr;
    valid = report(actionable != nullptr,
                   "candidate forgetting is not reachable from Fcitx") &&
            valid;
    const bool hasForgetAction =
        actionable != nullptr &&
        actionable->hasAction(candidates->candidate(0));
    valid = report(hasForgetAction, "candidate has no Fcitx forget action") &&
            valid;
    const auto candidateActions =
        hasForgetAction ? actionable->candidateActions(candidates->candidate(0))
                        : std::vector<fcitx::CandidateAction>{};
    const auto forgetAction = std::find_if(
        candidateActions.begin(), candidateActions.end(),
        [](const auto &action) { return action.text() == "Forget"; });
    const auto typoAction = std::find_if(
        candidateActions.begin(), candidateActions.end(),
        [](const auto &action) { return action.text() == "Correct typos"; });
    valid = report(forgetAction != candidateActions.end(),
                   "candidate forget action is missing") &&
            valid;
    valid = report(typoAction != candidateActions.end(),
                   "enabled LM typo correction has no candidate action") &&
            valid;
    if (forgetAction != candidateActions.end()) {
      actionable->triggerAction(candidates->candidate(0), forgetAction->id());
    }
    if (typoAction != candidateActions.end()) {
      actionable->triggerAction(candidates->candidate(0), typoAction->id());
    }

    const auto typoCandidates = inputContext.inputPanel().candidateList();
    valid =
        report(typoCandidates && typoCandidates->size() == 1,
               "LM typo corrections did not reach the Fcitx candidate UI") &&
        valid;
    valid = report(typoCandidates && typoCandidates->layoutHint() ==
                                         fcitx::CandidateLayoutHint::Vertical,
                   "LM typo corrections did not request a vertical layout") &&
            valid;
    if (typoCandidates && typoCandidates->size() == 1) {
      valid = report(typoCandidates->candidate(0).text().toString() == "仮名",
                     "LM typo correction displayed the wrong conversion") &&
              valid;
      valid =
          report(typoCandidates->candidate(0).comment().toString() == "かな",
                 "LM typo correction omitted the corrected input") &&
          valid;
    }
    fcitx::KeyEvent selectTypo(&inputContext, fcitx::Key(FcitxKey_1));
    engine.keyEvent(entry, selectTypo);
    valid = report(selectTypo.accepted(),
                   "selected LM typo correction was not accepted") &&
            valid;
    valid = report(inputContext.committed() == "仮名",
                   "selected LM typo correction was not committed") &&
            valid;

    fcitx::KeyEvent printableAgain(&inputContext, fcitx::Key(FcitxKey_a));
    engine.keyEvent(entry, printableAgain);
    valid = report(printableAgain.accepted(),
                   "second consumed printable key was not accepted") &&
            valid;
    fcitx::KeyEvent pageDown(&inputContext, fcitx::Key(FcitxKey_Page_Down));
    engine.keyEvent(entry, pageDown);
    valid = report(!pageDown.accepted(),
                   "unsupported Page Down was consumed by the addon") &&
            valid;
    const auto secondPage = inputContext.inputPanel().candidateList();
    const auto *pageable = secondPage ? secondPage->toPageable() : nullptr;
    valid = report(pageable && pageable->currentPage() == 0,
                   "sliding candidate window unexpectedly paged") &&
            valid;
    valid = report(secondPage && secondPage->size() == 9,
                   "sliding candidate window has the wrong size") &&
            valid;
    if (secondPage && secondPage->size() == 9) {
      valid = report(secondPage->candidate(0).text().toString() == "司会",
                     "unsupported Page Down changed the first candidate") &&
              valid;
      valid =
          report(secondPage->candidate(8).text().toString() == "Candidate 9",
                 "unsupported Page Down changed the last candidate") &&
          valid;
      valid = report(secondPage->cursorIndex() == 0,
                     "unsupported Page Down changed the cursor") &&
              valid;
    }
    fcitx::KeyEvent up(&inputContext, fcitx::Key(FcitxKey_Up));
    engine.keyEvent(entry, up);
    valid = report(up.accepted(), "consumed Up was not accepted") && valid;
    const auto retainedWindow = inputContext.inputPanel().candidateList();
    valid = report(retainedWindow && retainedWindow->size() == 9,
                   "candidate window changed size while moving within it") &&
            valid;
    if (retainedWindow && retainedWindow->size() == 9) {
      valid = report(retainedWindow->candidate(0).text().toString() == "司会",
                     "candidate window did not preserve its visible start") &&
              valid;
      valid = report(retainedWindow->cursorIndex() == 8,
                     "candidate window did not move its cursor upward") &&
              valid;
    }
    fcitx::KeyEvent selectFirstVisible(&inputContext, fcitx::Key(FcitxKey_1));
    engine.keyEvent(entry, selectFirstVisible);
    valid = report(selectFirstVisible.accepted(),
                   "first visible candidate selection was not accepted") &&
            valid;
    valid = report(inputContext.committed() == "仮名司会",
                   "visible candidate number selected the wrong candidate") &&
            valid;
    valid = report(inputContext.inputPanel().clientPreedit().empty(),
                   "committed preedit was not cleared") &&
            valid;
    valid = report(inputContext.inputPanel().candidateList() == nullptr,
                   "post-composition candidates must stay hidden") &&
            valid;

    const auto beforePrediction = inputContext.committed();
    fcitx::KeyEvent predictionInput(&inputContext, fcitx::Key(FcitxKey_p));
    engine.keyEvent(entry, predictionInput);
    valid = report(predictionInput.accepted() &&
                       inputContext.inputPanel().clientPreedit().toString() ==
                           "くだし" &&
                       inputContext.inputPanel().auxDown().toString() ==
                           "→ 下さい" &&
                       !inputContext.inputPanel().candidateList() &&
                       inputContext.committed() == beforePrediction,
                   "prediction-only state did not preserve the marked reading") &&
            valid;
    fcitx::KeyEvent tab(&inputContext, fcitx::Key(FcitxKey_Tab));
    engine.keyEvent(entry, tab);
    valid = report(tab.accepted() &&
                       inputContext.inputPanel().clientPreedit().toString() ==
                           "ください" &&
                       inputContext.inputPanel().auxDown().empty() &&
                       inputContext.committed() == beforePrediction,
                   "native Tab must update corrected preedit without a commit") &&
            valid;
    fcitx::KeyEvent commitPrediction(&inputContext, fcitx::Key(FcitxKey_Return));
    engine.keyEvent(entry, commitPrediction);
    valid = report(commitPrediction.accepted() &&
                       inputContext.committed() == beforePrediction + "ください" &&
                       inputContext.inputPanel().clientPreedit().empty() &&
                       inputContext.inputPanel().auxDown().empty(),
                   "Enter must commit accepted prediction exactly once") &&
            valid;

    inputContext.setCapabilityFlags(fcitx::CapabilityFlags(
        {fcitx::CapabilityFlag::Preedit,
         fcitx::CapabilityFlag::CommitStringWithCursor}));
    for (int selection = 0; selection < 3; ++selection) {
      const auto expectedCommit = inputContext.committed() + "[]";
      fcitx::KeyEvent bracketInput(&inputContext, fcitx::Key(FcitxKey_b));
      engine.keyEvent(entry, bracketInput);
      valid =
          report(bracketInput.accepted(), "bracket input was not accepted") &&
          valid;
      if (selection == 2) {
        const auto bracketCandidates =
            inputContext.inputPanel().candidateList();
        valid = report(bracketCandidates && bracketCandidates->size() > 0,
                       "bracket candidates are missing") &&
                valid;
        if (bracketCandidates && bracketCandidates->size() > 0) {
          bracketCandidates->candidate(0).select(&inputContext);
        }
      } else {
        fcitx::KeyEvent commitBracket(
            &inputContext,
            fcitx::Key(selection == 0 ? FcitxKey_Return : FcitxKey_1));
        engine.keyEvent(entry, commitBracket);
        valid = report(commitBracket.accepted(),
                       "bracket candidate selection was not accepted") &&
                valid;
      }
      valid = report(inputContext.committed() == expectedCommit &&
                         inputContext.commitCursor() == 2 &&
                         !inputContext.usedCursorCommit(),
                     "bracket candidate must commit normally at the end") &&
              valid;
      valid = report(inputContext.inputPanel().clientPreedit().empty() &&
                         !inputContext.inputPanel().candidateList(),
                     "bracket commit must clear composition UI") &&
              valid;
    }

    if (!statusActions.empty()) {
      statusActions.front()->activate(&inputContext);
    }

    TestInputContext resumedContext(instance.inputContextManager());
    TestInputContext resetContext(instance.inputContextManager());
    TestInputContext switchedContext(instance.inputContextManager());
    for (auto *context : {&resumedContext, &resetContext, &switchedContext}) {
      context->setCapabilityFlags(fcitx::CapabilityFlag::Preedit);
      fcitx::KeyEvent input(context, fcitx::Key(FcitxKey_a));
      engine.keyEvent(entry, input);
      valid =
          report(input.accepted() &&
                     context->inputPanel().clientPreedit().toString() ==
                         "かな" &&
                     context->inputPanel().candidateList(),
                 "shared connection did not start an independent session") &&
          valid;
    }

    fcitx::KeyEvent invalidResponse(&inputContext, fcitx::Key(FcitxKey_b));
    engine.keyEvent(entry, invalidResponse);
    valid = report(!invalidResponse.accepted(),
                   "a mismatched daemon response consumed the key") &&
            valid;
    valid = report(inputContext.inputPanel().clientPreedit().empty(),
                   "a mismatched daemon response left stale preedit") &&
            valid;
    valid = report(inputContext.inputPanel().candidateList() == nullptr,
                   "a mismatched daemon response left stale candidates") &&
            valid;

    fcitx::KeyEvent recoveredInput(&inputContext, fcitx::Key(FcitxKey_a));
    engine.keyEvent(entry, recoveredInput);
    valid = report(recoveredInput.accepted() &&
                       inputContext.inputPanel().clientPreedit().toString() ==
                           "かな",
                   "first fresh key was lost after reconnecting the session") &&
            valid;
    fcitx::KeyEvent resumedInput(&resumedContext, fcitx::Key(FcitxKey_a));
    engine.keyEvent(entry, resumedInput);
    valid =
        report(resumedInput.accepted() &&
                   resumedContext.inputPanel().clientPreedit().toString() ==
                       "かな",
               "another context reused its old session after reconnecting") &&
        valid;

    fcitx::InputContextEvent resetEvent(&resetContext,
                                        fcitx::EventType::InputContextReset);
    fcitx::InputContextEvent switchEvent(
        &switchedContext, fcitx::EventType::InputContextSwitchInputMethod);
    for (auto *context : {&resetContext, &switchedContext}) {
      if (context == &resetContext) {
        engine.reset(entry, resetEvent);
      } else {
        engine.deactivate(entry, switchEvent);
      }
      valid = report(context->inputPanel().clientPreedit().empty() &&
                         !context->inputPanel().candidateList() &&
                         context->inputPanel().auxDown().empty() &&
                         context->statusArea()
                             .actions(fcitx::StatusGroup::InputMethod)
                             .empty() &&
                         context->committed().empty(),
                     "stale lifecycle callback did not discard its local UI") &&
              valid;
      fcitx::KeyEvent continuingInput(&inputContext, fcitx::Key(FcitxKey_a));
      engine.keyEvent(entry, continuingInput);
      valid =
          report(continuingInput.accepted(),
                 "stale lifecycle callback broke the recovered connection") &&
          valid;
    }
    fcitx::InputContextEvent resumedOut(&resumedContext,
                                        fcitx::EventType::InputContextFocusOut);
    engine.deactivate(entry, resumedOut);
    fcitx::InputContextEvent focusOut(&inputContext,
                                      fcitx::EventType::InputContextFocusOut);
    engine.deactivate(entry, focusOut);
  }

  server.join();
  valid = report(serverValid, "addon sent an unexpected IPC request") && valid;
  valid = report(unlink(socketPath.c_str()) == 0,
                 "failed to remove daemon socket") &&
          valid;
  valid = report(rmdir(beanKeyDirectory.c_str()) == 0,
                 "failed to remove beankey runtime directory") &&
          valid;
  valid = report(rmdir(runtimeDirectory) == 0,
                 "failed to remove runtime directory") &&
          valid;
  return valid ? 0 : 1;
}
