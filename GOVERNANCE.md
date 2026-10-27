# Governance

## Today

Siontology is maintained by **ColdAI**. ColdAI's maintainers review and
merge changes, decide what goes into the specification, and publish the
documentation at siontology.org. The maintainers are the members of the
team listed in [`.github/CODEOWNERS`](.github/CODEOWNERS).

Anyone can contribute by issue or pull request
([CONTRIBUTING.md](CONTRIBUTING.md)). Decisions are made in the open, on
the pull request or issue concerned, and the maintainer who decides says
why.

## Decisions about the specification

The state transition specification, `docs/spec/state-transition.md`, is the
normative description that both proof stacks implement. A change to it:

- is proposed as a pull request that changes the specification, raises its
  version and explains the change in its **Changes** section;
- lands together with the matching changes to the reference executor
  (`sion-exec`) and to the independent implementation (`sion-spec`), and
  with differential tests on which both agree;
- needs the approval of a maintainer, and should be reviewed by someone
  who did not write the change.

Changes that weaken a guarantee or add a trust assumption must also update
`docs/implementation-status.md`, which names every shortcut the
implementation takes.

## Disagreements

Raise them on the issue or pull request. If a disagreement cannot be
settled there, the maintainers decide and record the reasoning. A
maintainer does not merge a change they wrote alone when another
maintainer has objected.

## How this will change

This file describes a project with one maintaining organisation and no
production network. It will change as the project does, through pull
requests to this file. Two directions are planned:

- **Maintainers from outside ColdAI**, as contributors take on sustained
  responsibility for parts of the code.
- **Protocol governance for a network.** If a network is launched, changes
  to the protocol that its validators run will need safeguards against
  capture. The architecture names three: time-locked upgrades, approval by
  a supermajority of stake, and time for users to exit before a change
  takes effect. None of this applies to the reference implementation
  today.
