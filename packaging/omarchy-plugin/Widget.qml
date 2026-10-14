import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Ui

// Herdr Planner in the bar: one pill with the attention counters of the local
// herdr-planner service (GET /api/summary); a click opens a popup under it with
// the projects (GET /api/overview): those with something waiting on you or to
// review first, then the rest in the planner's groups. Choosing a project
// focuses its herdr workspace. The planner's page is never opened from here.
// While the service does not answer the pill is hidden, with nothing written
// to the log.
Panel {
  id: root
  moduleName: "salehardec.herdr-planner"
  // One handler per target: every monitor's bar builds its own copy of this
  // widget, so none registers an IPC target.
  manageIpc: false

  readonly property int port: Math.max(1024, Math.min(65535, Number(setting("port", 7117)) || 7117))
  readonly property int refreshSec: Math.max(2, Math.min(120, Number(setting("refreshSec", 5)) || 5))
  readonly property string baseUrl: "http://127.0.0.1:" + port

  // ---------- palette ----------
  readonly property color fg: root.barForeground
  readonly property color dim: Qt.darker(fg, 1.55)
  readonly property color accentC: Color.accent
  readonly property string fam: Style.font.family

  // ---------- pill state ----------
  property bool online: false
  property int waiting: 0
  property string waitingHint: ""
  property int review: 0
  property int inbox: 0
  property int projectCount: 0
  readonly property bool attention: waiting > 0 || review > 0

  // Loud while something needs you, an ordinary chip colour otherwise.
  readonly property color pillColor: attention ? accentC : Util.alpha(fg, 0.14)
  readonly property color onPill: attention ? Color.background : fg
  readonly property color chipColor: attention ? Color.background : Util.alpha(fg, 0.16)
  readonly property color onChip: attention ? accentC : fg

  readonly property string tooltip: {
    var parts = []
    parts.push(waiting > 0 ? "ждут тебя: " + waiting : "никто не ждёт")
    parts.push("ревью: " + review)
    if (inbox > 0) parts.push("входящие: " + inbox)
    // What exactly waits, the newest first: «Alpha: координатор ждёт ответа».
    return "Herdr Planner · " + parts.join(" · ") + (waitingHint !== "" ? "\n" + waitingHint : "")
  }

  // Horizontal margins make the gap between pills; the height is the bar's.
  readonly property real pillMargin: Style.space(3)
  readonly property real slotHeight: root.bar ? root.bar.barSize : Style.space(26)
  visible: online
  opacity: online ? 1 : 0
  implicitWidth: online ? pill.implicitWidth + pillMargin * 2 : 0
  implicitHeight: slotHeight || Style.space(26)
  // What the bar asks before it shows a tooltip for this widget.
  readonly property bool tooltipHovered: area.containsMouse && !root.opened

  Behavior on opacity {
    NumberAnimation { duration: 160; easing.type: Easing.OutCubic }
  }

  // ---------- requests ----------
  // Connection refused, a timeout or a non-200 answer all mean "not there":
  // XMLHttpRequest reports them as status 0 without writing a warning.
  function request(method, path, body, done) {
    var xhr = new XMLHttpRequest()
    xhr.timeout = 2500
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      done(xhr.status, xhr.responseText)
    }
    xhr.open(method, root.baseUrl + path)
    if (body !== undefined) xhr.setRequestHeader("Content-Type", "application/json")
    xhr.send(body === undefined ? undefined : JSON.stringify(body))
  }

  property bool summaryBusy: false

  function applySummary(data) {
    if (!data || typeof data.waiting !== "number" || typeof data.review !== "number") return false
    waiting = data.waiting
    waitingHint = typeof data.waiting_hint === "string" ? data.waiting_hint : ""
    review = data.review
    inbox = Number(data.inbox) || 0
    projectCount = Number(data.projects) || 0
    return true
  }

  function pollSummary() {
    if (summaryBusy) return
    summaryBusy = true
    request("GET", "/api/summary", undefined, function(status, text) {
      summaryBusy = false
      var data = null
      if (status === 200) { try { data = JSON.parse(text) } catch (e) { data = null } }
      online = applySummary(data)
    })
  }

  Component.onCompleted: pollSummary()
  onPortChanged: pollSummary()

  Timer {
    // Down: look again less often, but soon enough to show up after a restart.
    interval: root.online ? root.refreshSec * 1000 : Math.max(10, root.refreshSec) * 1000
    running: true
    repeat: true
    onTriggered: root.pollSummary()
  }

  // ---------- popup data ----------
  property var overview: null
  property string overviewJson: ""
  property bool overviewBusy: false
  // Folded sections by key (the popup's own, until the shell restarts).
  property var folded: ({})
  // «5 мин» is counted from this; the poll moves it even when the data is unchanged.
  property double nowMs: Date.now()
  property var rows: []
  property int cursor: -1
  property string cursorKey: ""

  function plural(n, one, few, many) {
    var m10 = n % 10, m100 = n % 100
    if (m10 === 1 && m100 !== 11) return one
    if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few
    return many
  }

  // A short «how long ago» for a unix-millisecond time.
  function ago(ms) {
    if (!ms || ms <= 0) return ""
    var min = Math.floor(Math.max(0, nowMs - ms) / 60000)
    if (min < 1) return "сейчас"
    if (min < 60) return min + " мин"
    if (min < 1440) return Math.floor(min / 60) + " ч"
    return Math.floor(min / 1440) + " дн"
  }

  function pollOverview() {
    if (overviewBusy) return
    overviewBusy = true
    request("GET", "/api/overview", undefined, function(status, text) {
      overviewBusy = false
      if (status !== 200) { online = false; return }
      nowMs = Date.now()
      if (text === overviewJson) return
      var data = null
      try { data = JSON.parse(text) } catch (e) { return }
      if (!data || !Array.isArray(data.waiting) || !Array.isArray(data.review) || !Array.isArray(data.working)) return
      overviewJson = text
      overview = data
      if (data.summary) applySummary(data.summary)
      rebuildRows()
    })
  }

  // The flat list the cursor walks: the sections that are not empty, each with its projects.
  // Projects where nothing happens are not rows: the popup only counts them.
  function rebuildRows() {
    // «Show every project» is always the first row: herdr gives no way to ask
    // whether its sidebar is filtered, so it is offered whenever the list is.
    var out = [
      { kind: "action", key: "__unfocus", title: "Показать все проекты", sub: "сбросить фильтр боковой панели herdr (unfocus)", letter: "U" },
      { kind: "action", key: "__open", title: "Полный планировщик", sub: "вкладка herdr-gpui или окно приложения", letter: "O" }
    ]
    var ov = overview
    if (ov) {
      var open = function(key) { return folded[key] !== true }
      var sections = [
        { key: "__waiting", label: "\uf0f3  Ждут тебя", list: ov.waiting, hot: true },
        { key: "__review", label: "\uf06e  На ревью", list: ov.review, hot: false },
        { key: "__working", label: "\uf013  Работают", list: ov.working, hot: false }
      ]
      for (var s = 0; s < sections.length; s++) {
        var sec = sections[s]
        if (sec.list.length === 0) continue
        out.push({ kind: "group", key: sec.key, label: sec.label, count: sec.list.length, open: open(sec.key), hot: sec.hot })
        if (!open(sec.key)) continue
        for (var p = 0; p < sec.list.length; p++)
          out.push({ kind: "project", key: sec.list[p].slug, project: sec.list[p], group: sec.key, section: sec.key })
      }
    }
    rows = out
    var at = -1
    for (var i = 0; i < out.length; i++) if (out[i].key === cursorKey) { at = i; break }
    if (at < 0) at = Math.min(Math.max(cursor, firstProject()), out.length - 1)
    setCursor(at)
  }

  // The row a fresh opening starts on: the first project, not the action above it.
  function firstProject() {
    for (var i = 0; i < rows.length; i++) if (rows[i].kind !== "action") return i
    return 0
  }

  function setCursor(index) {
    cursor = index
    cursorKey = index >= 0 && index < rows.length ? rows[index].key : ""
  }

  function moveCursor(delta) {
    if (rows.length === 0) return
    setCursor(Math.max(0, Math.min(rows.length - 1, (cursor < 0 ? (delta > 0 ? -1 : rows.length) : cursor) + delta)))
  }

  function toggleGroup(key, want) {
    var map = Object.assign({}, folded)
    var isOpen = map[key] !== true
    if (want === undefined) want = !isOpen
    if (want) delete map[key]
    else map[key] = true
    folded = map
    rebuildRows()
  }

  function activate(index) {
    if (index < 0 || index >= rows.length) return
    var row = rows[index]
    if (row.kind === "group") toggleGroup(row.key)
    else if (row.kind === "action") (row.key === "__open" ? openPlanner() : unfocusAll())
    else focusProject(row.project)
  }

  function moveSideways(dx) {
    if (cursor < 0 || cursor >= rows.length) return
    var row = rows[cursor]
    if (row.kind === "group") toggleGroup(row.key, dx > 0)
    else if (dx < 0 && row.group !== "") {
      for (var i = cursor - 1; i >= 0; i--)
        if (rows[i].kind === "group" && rows[i].key === row.group) { setCursor(i); return }
    }
  }

  // `herdr-projects focus <slug>` through the planner service, as the F key does on
  // its page. The popup closes at once; a refusal comes back as a notification.
  function sendAction(body, failure) {
    close()
    request("POST", "/api/action", body, function(status, text) {
      if (status === 200) return
      var why = ""
      try { why = String(JSON.parse(text).error || "") } catch (e) { why = "" }
      Quickshell.execDetached(["omarchy-notification-send", "-u", "low",
        "Herdr Planner: " + failure + (why !== "" ? ": " + why : "")])
    })
  }

  function focusProject(project) {
    sendAction({ kind: "focus", slug: project.slug }, "не удалось перейти к «" + String(project.name || project.slug) + "»")
  }

  // The full page: `herdr-planner-open` tries a herdr-gpui tab (when that build has
  // tabs), else an Omarchy app window. Never the plain system browser, never a click's
  // side effect on the page. Run by the shell itself, so it has the desktop's environment.
  function openPlanner() {
    close()
    var home = String(Quickshell.env("HOME") || "")
    Quickshell.execDetached(["sh", "-c",
      "exec \"$1/.local/bin/herdr-planner-open\" 2>/dev/null || exec herdr-planner-open", "sh", home])
  }

  // `herdr-projects unfocus`: the sidebar lists every project again.
  function unfocusAll() {
    sendAction({ kind: "unfocus" }, "не удалось показать все проекты")
  }

  onOpenedChanged: {
    if (opened) {
      // Every opening starts at the top with the first row under the cursor.
      flick.contentY = 0
      setCursor(rows.length > 0 ? firstProject() : -1)
      pollOverview()
      refreshTimer.restart()
    } else {
      refreshTimer.stop()
    }
  }
  onOnlineChanged: if (!online && opened) close()
  onBarChanged: if (!bar && opened) close()

  Timer {
    id: refreshTimer
    interval: root.refreshSec * 1000
    repeat: true
    onTriggered: root.pollOverview()
  }

  // ---------- pill ----------
  Item {
    id: pill
    implicitWidth: content.implicitWidth + Style.space(18)
    implicitHeight: Math.max(Style.space(20), root.slotHeight - Style.space(6))
    anchors.centerIn: parent

    Rectangle {
      anchors.fill: parent
      radius: Style.space(8)
      color: root.pillColor
      opacity: area.containsMouse || root.opened ? 1 : 0.92
      Behavior on opacity { NumberAnimation { duration: 120 } }
      Behavior on color { ColorAnimation { duration: 160 } }
    }

    RowLayout {
      id: content
      anchors.centerIn: parent
      spacing: Style.space(7)

      Text {
        textFormat: Text.PlainText
        text: "\uf0ae"
        color: root.onPill
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
        Layout.alignment: Qt.AlignVCenter
      }
      Text {
        textFormat: Text.PlainText
        text: "Plan"
        color: root.onPill
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
        font.bold: true
        Layout.alignment: Qt.AlignVCenter
      }
      Repeater {
        // Waiting on you first, then review; a zero shows no chip at all.
        model: [
          { glyph: "\uf0f3", count: root.waiting },
          { glyph: "\uf06e", count: root.review }
        ]
        delegate: Rectangle {
          required property var modelData
          visible: modelData.count > 0
          Layout.preferredWidth: visible ? counter.implicitWidth + Style.space(10) : 0
          Layout.preferredHeight: Style.space(15)
          Layout.alignment: Qt.AlignVCenter
          radius: Style.space(8)
          color: root.chipColor
          Text {
            id: counter
            anchors.centerIn: parent
            textFormat: Text.PlainText
            text: modelData.glyph + " " + modelData.count
            color: root.onChip
            font.family: root.fam
            font.pixelSize: Style.font.caption
            font.bold: true
          }
        }
      }
    }
  }

  MouseArea {
    id: area
    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onEntered: if (root.bar) root.bar.showTooltip(root, root.tooltip)
    onExited: if (root.bar) root.bar.hideTooltip(root)
    onClicked: {
      if (root.bar) root.bar.hideTooltip(root)
      root.toggle()
    }
  }

  // ---------- popup ----------
  KeyboardPanel {
    id: panel
    anchorItem: pill
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(420))
    contentHeight: panel.fittedContentHeight(heroAndList.implicitHeight, Style.space(600))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: !keyCatcher.activeFocus

      onCloseRequested: root.close()
      onActivateRequested: root.activate(root.cursor)
      onMoveRequested: function(dx, dy) {
        if (dy !== 0) root.moveCursor(dy)
        else root.moveSideways(dx)
      }
      onTextKey: function(t) {
        if (t === "r") root.pollOverview()
        else if (t === "o") root.openPlanner()
        else if (t === "u" || t === "0") root.unfocusAll()
      }

      Column {
        id: heroAndList
        anchors.left: parent.left
        anchors.right: parent.right
        spacing: Style.space(8)

        PanelHero {
          width: parent.width
          title: "Herdr Planner"
          meta: root.waiting + " ждут тебя · " + root.review + " на ревью"
            + (root.inbox > 0 ? " · " + root.inbox + " вх." : "")
          foreground: root.fg
          fontFamily: root.fam
          iconComponent: Component {
            Text {
              textFormat: Text.PlainText
              text: "\uf0ae"
              color: root.fg
              font.family: root.fam
              font.pixelSize: Style.font.display
            }
          }
        }

        Flickable {
          id: flick
          width: parent.width
          height: Math.min(listColumn.implicitHeight, Style.space(480))
          contentWidth: width
          contentHeight: listColumn.implicitHeight
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          flickableDirection: Flickable.VerticalFlick
          interactive: contentHeight > height
          ScrollBar.vertical: ScrollBar {
            policy: flick.contentHeight > flick.height ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff
          }

          Column {
            id: listColumn
            width: flick.width - (flick.contentHeight > flick.height ? Style.space(10) : 0)
            spacing: Style.space(1)

            Repeater {
              model: root.rows

              delegate: PlannerRow {
                required property var modelData
                required property int index
                width: listColumn.width
                row: modelData
                cursorSelected: index === root.cursor
                onHoverCursor: root.setCursor(index)
                onActivated: root.activate(index)
              }
            }

            // Nothing waits, nothing is to review, nothing works.
            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: root.overview !== null && root.rows.length <= 1 && root.overview.quiet > 0
              text: "Всё тихо"
              color: root.dim
              font.family: root.fam
              font.pixelSize: Style.font.bodySmall
              horizontalAlignment: Text.AlignHCenter
              topPadding: Style.space(10)
              bottomPadding: Style.space(4)
            }
          }
        }

        // Not a row: no cursor, no click.
        Text {
          textFormat: Text.PlainText
          width: parent.width
          visible: root.overview !== null && root.overview.quiet > 0
          text: root.overview === null ? "" : "ещё " + root.overview.quiet + " "
            + root.plural(root.overview.quiet, "проект", "проекта", "проектов") + " без активности"
          color: root.dim
          font.family: root.fam
          font.pixelSize: Style.font.caption
          horizontalAlignment: Text.AlignHCenter
          topPadding: Style.space(2)
          bottomPadding: Style.space(2)
        }

        Text {
          textFormat: Text.PlainText
          width: parent.width
          visible: root.overview === null || (root.overview.summary && root.overview.summary.projects === 0)
          text: root.overview === null ? "Загрузка…" : "Нет проектов"
          color: root.dim
          font.family: root.fam
          font.pixelSize: Style.font.caption
          horizontalAlignment: Text.AlignHCenter
          topPadding: Style.space(10)
          bottomPadding: Style.space(10)
        }

        Text {
          textFormat: Text.PlainText
          width: parent.width
          text: "↑↓ jk · Enter перейти · U показать все · O планировщик · ←→ свернуть · Esc закрыть"
          color: root.dim
          font.family: root.fam
          font.pixelSize: Style.font.caption
          horizontalAlignment: Text.AlignRight
          elide: Text.ElideLeft
        }
      }
    }
  }

  // ---------- row ----------
  component PlannerRow: CursorSurface {
    id: line

    property var row: null
    property bool cursorSelected: false
    readonly property bool isGroup: row !== null && row.kind === "group"
    readonly property bool isAction: row !== null && row.kind === "action"
    readonly property var project: row !== null && row.kind === "project" ? row.project : null
    readonly property int total: project ? project.waiting + project.review + project.working + project.blocked + project.resolved : 0
    signal hoverCursor()
    signal activated()

    hasCursor: cursorSelected
    foreground: root.fg
    accent: root.accentC
    bordered: isAction
    implicitHeight: isGroup ? Math.max(Style.space(24), groupRow.implicitHeight + Style.space(6))
      : (isAction ? Math.max(Style.space(30), actionRow.implicitHeight + Style.space(10))
        : Math.max(Style.space(34), projectRow.implicitHeight + Style.space(8)))

    onCursorSelectedChanged: {
      if (!cursorSelected) return
      var top = line.mapToItem(flick.contentItem, 0, 0).y
      var bottom = top + line.height
      if (top < flick.contentY) flick.contentY = top
      else if (bottom > flick.contentY + flick.height) flick.contentY = bottom - flick.height
    }

    // Only a moving pointer takes the cursor: rows rebuilt under a resting
    // pointer must not steal it from the keyboard.
    HoverHandler {
      onPointChanged: line.hoverCursor()
    }
    MouseArea {
      anchors.fill: parent
      cursorShape: Qt.PointingHandCursor
      onClicked: line.activated()
    }

    // ---- «show every project» ----
    Rectangle {
      anchors.fill: parent
      radius: line.radius
      visible: line.isAction
      color: Util.alpha(root.accentC, 0.12)
    }
    RowLayout {
      id: actionRow
      visible: line.isAction
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10)
      anchors.rightMargin: Style.space(10)
      spacing: Style.space(8)

      Text {
        textFormat: Text.PlainText
        text: "\uf00a"
        color: root.accentC
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
        Layout.alignment: Qt.AlignVCenter
      }
      Column {
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter
        spacing: Style.space(1)
        Text {
          textFormat: Text.PlainText
          width: parent.width
          text: line.row && line.row.title ? String(line.row.title) : ""
          color: root.accentC
          font.family: root.fam
          font.pixelSize: Style.font.bodySmall
          font.bold: true
          elide: Text.ElideRight
        }
        Text {
          textFormat: Text.PlainText
          width: parent.width
          text: line.row && line.row.sub ? String(line.row.sub) : ""
          color: root.dim
          font.family: root.fam
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }
      Text {
        textFormat: Text.PlainText
        text: line.row && line.row.letter ? String(line.row.letter) : ""
        color: root.accentC
        font.family: root.fam
        font.pixelSize: Style.font.caption
        font.bold: true
        Layout.alignment: Qt.AlignVCenter
      }
    }

    // ---- group header ----
    RowLayout {
      id: groupRow
      visible: line.isGroup
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(8)
      anchors.rightMargin: Style.space(8)
      spacing: Style.space(6)

      Text {
        textFormat: Text.PlainText
        text: line.isGroup && line.row.open ? "▾" : "▸"
        color: line.isGroup && line.row.hot ? root.accentC : root.dim
        font.family: root.fam
        font.pixelSize: Style.font.caption
        Layout.alignment: Qt.AlignVCenter
      }
      Text {
        textFormat: Text.PlainText
        text: line.isGroup ? String(line.row.label).toUpperCase() : ""
        color: line.isGroup && line.row.hot ? root.accentC : root.dim
        font.family: root.fam
        font.pixelSize: Style.font.caption
        font.bold: true
        elide: Text.ElideRight
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter
      }
      Text {
        textFormat: Text.PlainText
        text: line.isGroup ? String(line.row.count) : ""
        color: root.dim
        font.family: root.fam
        font.pixelSize: Style.font.caption
        Layout.alignment: Qt.AlignVCenter
      }
    }

    // ---- project ----
    RowLayout {
      id: projectRow
      visible: line.project !== null
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(14)
      anchors.rightMargin: Style.space(8)
      spacing: Style.space(8)

      Column {
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter
        spacing: Style.space(3)

        Text {
          textFormat: Text.PlainText
          width: parent.width
          text: line.project ? String(line.project.name) : ""
          color: root.fg
          font.family: root.fam
          font.pixelSize: Style.font.bodySmall
          font.bold: line.cursorSelected
          elide: Text.ElideRight
        }

        // What waits, in words (only for projects that wait on the user).
        Text {
          textFormat: Text.PlainText
          width: parent.width
          visible: line.project !== null && line.project.wait_hint !== ""
          text: line.project ? String(line.project.wait_hint) : ""
          color: root.accentC
          font.family: root.fam
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }

        // Progress: threads done out of all threads of the project.
        Row {
          width: parent.width
          spacing: Style.space(6)
          visible: line.total > 0 && !(line.project !== null && line.project.wait_hint !== "")

          Rectangle {
            id: track
            width: parent.width - doneText.implicitWidth - parent.spacing
            height: Style.space(3)
            radius: height / 2
            anchors.verticalCenter: parent.verticalCenter
            color: Util.alpha(root.fg, 0.14)

            Rectangle {
              width: line.total > 0 ? Math.max(height, Math.round(track.width * line.project.resolved / line.total)) : 0
              height: parent.height
              radius: parent.radius
              color: Util.alpha(root.accentC, 0.85)
              visible: line.project !== null && line.project.resolved > 0
            }
          }
          Text {
            id: doneText
            textFormat: Text.PlainText
            text: line.project ? line.project.resolved + "/" + line.total : ""
            color: root.dim
            font.family: root.fam
            font.pixelSize: Style.font.caption
          }
        }
      }

      // When the newest event that needs a look arrived.
      Text {
        textFormat: Text.PlainText
        visible: text !== ""
        text: line.project && (line.project.waiting > 0 || line.project.review > 0) ? root.ago(line.project.attention_since) : ""
        color: root.dim
        font.family: root.fam
        font.pixelSize: Style.font.caption
        Layout.alignment: Qt.AlignVCenter
      }

      // Counters: waiting on you, to review, working, stuck. A zero is not drawn.
      Repeater {
        model: line.project ? [
          { glyph: "\uf0f3", count: line.project.waiting, tone: "hot" },
          { glyph: "\uf06e", count: line.project.review, tone: "chip" },
          { glyph: "\uf013", count: line.project.working, tone: "plain" },
          { glyph: "\uf071", count: line.project.blocked, tone: "bad" }
        ] : []

        delegate: Rectangle {
          required property var modelData
          visible: modelData.count > 0
          Layout.preferredWidth: visible ? mark.implicitWidth + Style.space(10) : 0
          Layout.preferredHeight: Style.space(16)
          Layout.alignment: Qt.AlignVCenter
          radius: Style.space(8)
          color: modelData.tone === "hot" ? root.accentC
            : (modelData.tone === "chip" ? Util.alpha(root.fg, 0.16) : "transparent")

          Text {
            id: mark
            anchors.centerIn: parent
            textFormat: Text.PlainText
            text: modelData.glyph + " " + modelData.count
            color: modelData.tone === "hot" ? Color.background
              : (modelData.tone === "bad" ? Color.urgent : (modelData.tone === "plain" ? root.dim : root.fg))
            font.family: root.fam
            font.pixelSize: Style.font.caption
            font.bold: modelData.tone === "hot"
          }
        }
      }
    }
  }
}
