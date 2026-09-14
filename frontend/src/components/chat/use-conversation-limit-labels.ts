import { useTranslation } from "react-i18next"
import type { ConversationLimitLabels } from "./conversation-limit-notice"

/**
 * Both surfaces render the notice from the `student` namespace (the
 * embed view borrows it), so the label set is resolved once here.
 */
export function useConversationLimitLabels(): ConversationLimitLabels {
  const { t } = useTranslation("student")
  return {
    topicTitle: t("limit.topicTitle"),
    topicBody: t("limit.topicBody"),
    nudgeTitle: t("limit.nudgeTitle"),
    nudgeBody: t("limit.nudgeBody"),
    nudgeTopicBody: t("limit.nudgeTopicBody"),
    blockedTitle: t("limit.blockedTitle"),
    blockedBody: t("limit.blockedBody"),
    blockedTopicBody: t("limit.blockedTopicBody"),
    continueAction: t("limit.continueAction"),
    continueWorking: t("limit.continueWorking"),
    newChatAction: t("limit.newChatAction"),
    dismiss: t("limit.dismiss"),
  }
}
