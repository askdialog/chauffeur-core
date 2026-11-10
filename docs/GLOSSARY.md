# Glossary

The words Chauffeur's code, interface and docs use for its domain. One word per concept; the others go under _Avoid_.

## Distribution

**Member**:
A user signed in to Chauffeur with a Google account of the distribution's member domain (`[distribution] member_domain`, `askdialog.com` in Dialog's build). Only members see the features that rely on the distribution's hosted services.
_Avoid_: internal user, Dialog user, employee

## Sharing

**Invitee**:
An e-mail address named on a design share or a workspace share. On a workspace share, an invitee has a role (viewer or suggest).
_Avoid_: reviewer, guest, recipient

**Team domain**:
The Google Workspace domain a share is open to, taken from its owner's account when they open it to their team; anyone signed in with a Google account of that domain may open the share.
_Avoid_: org, company, team (alone)

**Sign-in link**:
A single-use link, valid for 15 minutes, that the share service e-mails to an invitee who asks for it, and that signs them in as that address.
_Avoid_: magic link, login link
