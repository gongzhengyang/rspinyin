/*
 * rspinyin-test-client: the minimal commit-probe client FEAT-TEST-P0.02.02 specifies.
 *
 * A GTK3 window with one GtkEntry whose IM context forwards every `commit` and
 * `preedit-changed` signal to stdout as one JSON object per line, so the harness
 * (xtask testd-client) can assert what the host actually committed instead of
 * OCR-ing a screenshot. The fcitx5 GTK frontend module bridges the input context,
 * so the client needs no IM code of its own beyond the plumbing. The build is a
 * plain gcc call (see build.sh in this directory).
 *
 * Privacy: the text the host commits IS user input. This program only writes it
 * to the pipe its parent owns -- never to syslog, never to a file it opens, and
 * the parent (xtask testd-client) treats the transcript with the same 0600
 * discipline as every other artifact that holds what the user typed.
 */
#include <gtk/gtk.h>
#include <gdk/gdkx.h>
#include <stdio.h>

static void print_json_line(const char *event, const char *text, gint cursor) {
    /* Standard JSON string escaping: control characters as \uXXXX (and the two
     * byte-value shorthands), everything else verbatim. UTF-8 bytes above 0x7f
     * go out raw, which is valid JSON; g_strescape's octal form is not, and the
     * harness's strict parser refuses it. */
    printf("{\"event\":\"%s\",\"text\":\"", event);
    for (const gchar *p = text ? text : ""; *p; ++p) {
        guchar c = (guchar)*p;
        switch (c) {
        case '"': fputs("\\\"", stdout); break;
        case '\\': fputs("\\\\", stdout); break;
        case '\n': fputs("\\n", stdout); break;
        case '\r': fputs("\\r", stdout); break;
        case '\t': fputs("\\t", stdout); break;
        default:
            if (c < 0x20) {
                printf("\\u%04x", c);
            } else {
                putchar(c);
            }
        }
    }
    putchar('"');
    if (cursor >= 0) {
        printf(",\"cursor\":%d", cursor);
    }
    printf("}\n");
    fflush(stdout);
}

static void on_commit(GtkIMContext *ctx G_GNUC_UNUSED, gchar *text, gpointer data G_GNUC_UNUSED) {
    print_json_line("commit", text, -1);
}

static void report_preedit(GtkIMContext *ctx) {
    gchar *preedit = NULL;
    gint cursor = 0;
    /* gtk_im_context_get_preedit_string is the one call that reaches through the
     * multicontext to the delegate; the preedit-string property does not. */
    gtk_im_context_get_preedit_string(ctx, &preedit, NULL, &cursor);
    if (preedit && preedit[0] != '\0') {
        print_json_line("preedit", preedit, cursor);
    }
    g_free(preedit);
}

static void on_preedit(GtkIMContext *ctx, gpointer data G_GNUC_UNUSED) {
    report_preedit(ctx);
}

static gboolean focused = FALSE;

/* The activation-time dbus link is not up yet, so the focus_in issued at map
 * time is lost; re-issue it once the module is connected. Idempotent. */
static gboolean delayed_focus_in(gpointer data) {
    GtkIMContext *ctx = GTK_IM_CONTEXT(data);
    gtk_im_context_focus_in(ctx);
    focused = TRUE;
    return G_SOURCE_REMOVE;
}

static gboolean on_key_event(GtkWidget *widget, GdkEventKey *event, gpointer data) {
    GtkIMContext *ctx = GTK_IM_CONTEXT(data);
    GdkWindow *gdk = gtk_widget_get_window(widget);
    if (gdk) {
        gtk_im_context_set_client_window(ctx, gdk);
    }
    if (!focused) {
        /* The activation-time focus_in ran before the module's dbus link was up;
         * re-issue it on the first key once the client window is real. Idempotent. */
        gtk_im_context_focus_in(ctx);
        focused = TRUE;
    }
    gtk_im_context_filter_keypress(ctx, event);
    report_preedit(ctx);
    return FALSE;
}

static void on_activate(GtkApplication *app, gpointer data G_GNUC_UNUSED) {
    GtkWidget *window = gtk_application_window_new(app);
    gtk_window_set_title(GTK_WINDOW(window), "rspinyin-test-client");
    gtk_window_set_default_size(GTK_WINDOW(window), 420, 120);

    /* GTK3 hides a text widget's IM context behind version-specific getters, so
     * the probe owns its own multicontext and forwards every key event through
     * gtk_im_context_filter_keypress -- the same public path a real client's IM
     * module sits under. Its `commit` is what the host really committed. */
    GtkIMContext *ctx = gtk_im_multicontext_new();
    g_signal_connect(ctx, "commit", G_CALLBACK(on_commit), NULL);
    g_signal_connect(ctx, "preedit-changed", G_CALLBACK(on_preedit), NULL);
    g_object_set_data_full(G_OBJECT(window), "im-context", ctx, g_object_unref);

    g_signal_connect(window, "key-press-event", G_CALLBACK(on_key_event), ctx);
    g_signal_connect(window, "key-release-event", G_CALLBACK(on_key_event), ctx);

    gtk_widget_show_all(window);

    /* The host's controller Activate (and any key) must find this context
     * focused; 150 ms after map the immodule's dbus link is up. */
    g_timeout_add(150, delayed_focus_in, ctx);

    /* Report the mapped window id so the harness can focus it before injecting. */
    GdkWindow *gdk = gtk_widget_get_window(window);
    Window xid = GDK_WINDOW_XID(gdk);
    const gchar *caps = g_object_get_data(G_OBJECT(app), "rspinyin-caps");
    printf("{\"event\":\"ready\",\"caps\":\"%s\",\"window\":%lu}\n",
           caps ? caps : "", (unsigned long)xid);
    fflush(stdout);
}

int main(int argc, char **argv) {
    /* The probe's window id is the X11 window the injection and screenshot channels
     * target. A session can advertise both X11 and Wayland, but this client is the
     * X11 test endpoint, so GTK must not silently pick the Wayland backend. */
    g_setenv("GDK_BACKEND", "x11", TRUE);
    GtkApplication *app = gtk_application_new("dev.rspinyin.test-client",
                                              G_APPLICATION_NON_UNIQUE);
    g_signal_connect(app, "activate", G_CALLBACK(on_activate), NULL);
    const gchar *caps = "";
    gchar **gtk_args = g_new0(gchar *, (gsize)argc + 1);
    gsize gtk_argc = 0;
    for (int index = 1; index < argc; ++index) {
        if (g_str_has_prefix(argv[index], "--caps=")) {
            caps = argv[index] + sizeof("--caps=") - 1;
        } else if (!g_str_has_prefix(argv[index], "--display=")) {
            gtk_args[gtk_argc++] = argv[index];
        }
    }
    g_object_set_data_full(G_OBJECT(app), "rspinyin-caps",
                           g_strdup(caps ? caps : ""), g_free);
    int status = g_application_run(G_APPLICATION(app), (int)gtk_argc, gtk_args);
    g_free(gtk_args);
    g_object_unref(app);
    return status;
}
