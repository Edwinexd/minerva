// Cross-tab signal so the LTI Dynamic Registration popup can auto-close
// when the integrator finishes the approval flow in a separate tab.
//
// Flow:
//   1. LMS admin completes dynreg in the iframe; sees a success card with
//      an "Open Minerva to approve" link.
//   2. They click the link; a new tab loads /admin/lti-approve/<id>.
//   3. They (or another integrator on the same machine) approve.
//   4. The approve page posts {type:"approved", platformId} on this
//      channel.
//   5. The original dynreg popup is listening; when the platformId
//      matches, it fires the IMS-spec `org.imsglobal.lti.close`
//      postMessage so the LMS dismisses its dialog without the LMS admin
//      having to click Close manually.
//
// BroadcastChannel is same-origin only, which is exactly what we want;
// the popup and the approve page are both served from Minerva.

export const DYNREG_CHANNEL = "minerva-lti-dynreg"

export type DynregBroadcastMessage = {
  type: "approved"
  platformId: string
}
