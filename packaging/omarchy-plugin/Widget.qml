pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Ui

// Herdr Planner in the bar: one pill that opens the dashboard in its own app
// window. The dashboard itself is a separate, standalone web app; this widget
// does not draw a popup and does not need herdr or herdr-gpui.
//
// It probes GET /login (a public page) only to show whether the local service
// answers. Counts and task data stay behind the login, so they are not read here.
Panel {
  id: root
  moduleName: "salehardec.herdr-planner"
  manageIpc: false

  readonly property int port: Math.max(1024, Math.min(65535, Number(setting("port", 7117)) || 7117))
  readonly property int refreshSec: Math.max(2, Math.min(120, Number(setting("refreshSec", 10)) || 10))
  readonly property string baseUrl: "http://127.0.0.1:" + port
  // A TLS-enabled service still answers on loopback: try HTTPS after HTTP fails.
  property bool online: false
  property bool tls: false
  property bool probing: false
  property color fg: root.barForeground
  readonly property string fam: Style.font.family
  readonly property real pillMargin: Style.space(3)
  readonly property real slotHeight: root.bar ? root.bar.barSize : Style.space(26)

  visible: true
  implicitWidth: pill.implicitWidth + pillMargin * 2
  implicitHeight: slotHeight || Style.space(26)
  readonly property bool tooltipHovered: area.containsMouse
  readonly property string tooltip: "Herdr Planner · " + (online ? "открыть панель" : "сервис не отвечает — нажмите, чтобы запустить")

  function probe(scheme) {
    var xhr = new XMLHttpRequest()
    xhr.timeout = 2500
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      if (xhr.status >= 200 && xhr.status < 500) {
        root.online = true
        root.tls = (scheme === "https")
        root.probing = false
      } else if (scheme === "http") {
        probe("https")
      } else {
        root.online = false
        root.probing = false
      }
    }
    xhr.open("GET", scheme + "://127.0.0.1:" + root.port + "/login")
    xhr.send()
  }

  function poll() {
    if (probing) return
    probing = true
    probe("http")
  }

  Component.onCompleted: poll()
  onPortChanged: poll()

  Timer {
    interval: root.online ? root.refreshSec * 1000 : Math.max(10, root.refreshSec) * 1000
    running: true
    repeat: true
    onTriggered: root.poll()
  }

  // The Omarchy shell runs this, so it has the desktop's environment.
  function launchPlanner() {
    var home = String(Quickshell.env("HOME") || "")
    Quickshell.execDetached(["sh", "-c",
      "exec \"$1/.local/bin/herdr-planner-open\" 2>/dev/null || exec herdr-planner-open", "sh", home])
  }

  Item {
    id: pill
    implicitWidth: content.implicitWidth + Style.space(18)
    implicitHeight: Math.max(Style.space(20), root.slotHeight - Style.space(6))
    anchors.centerIn: parent

    Rectangle {
      anchors.fill: parent
      radius: Style.space(8)
      color: root.online ? Util.alpha(root.fg, 0.14) : Util.alpha(root.fg, 0.07)
      opacity: area.containsMouse ? 1 : 0.92
      Behavior on color { ColorAnimation { duration: 160 } }
      Behavior on opacity { NumberAnimation { duration: 120 } }
    }

    RowLayout {
      id: content
      anchors.centerIn: parent
      spacing: Style.space(7)

      // A small pixel planner mark: four cells, one filled.
      Row {
        spacing: 1
        Layout.alignment: Qt.AlignVCenter
        Repeater {
          model: [1, 1, 1, 0]
          delegate: Rectangle {
            required property int modelData
            width: Style.space(3)
            height: Style.space(3)
            color: modelData ? root.fg : "transparent"
          }
        }
      }
      Text {
        textFormat: Text.PlainText
        text: "Plan"
        color: root.fg
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
        font.bold: true
        Layout.alignment: Qt.AlignVCenter
      }
      Rectangle {
        Layout.preferredWidth: Style.space(6)
        Layout.preferredHeight: Style.space(6)
        radius: Style.space(3)
        color: root.online ? Color.accent : Util.alpha(root.fg, 0.35)
        Layout.alignment: Qt.AlignVCenter
        Behavior on color { ColorAnimation { duration: 160 } }
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
      root.launchPlanner()
    }
  }
}
