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
    g_autofree gchar *escaped = g_strescape(text ? text : "", NULL);
    if (cursor >= 0) {
        printf("{\"event\":\"%s\",\"text\":\"%s\",\"cursor\":%d}\n", event, escaped, cursor);
    } else {
        printf("{\"event\":\"%s\",\"text\":\"%s\"}\n", event, escaped);
    }
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

    /* Report the mapped window id so the harness can focus it before injecting. */
    GdkWindow *gdk = gtk_widget_get_window(window);
    Window xid = GDK_WINDOW_XID(gdk);
    printf("{\"event\":\"ready\",\"window\":\"0x%lx\"}\n", (unsigned long)xid);
    fflush(stdout);
}

int main(int argc, char **argv) {
    GtkApplication *app = gtk_application_new("dev.rspinyin.test-client",
                                              G_APPLICATION_NON_UNIQUE);
    g_signal_connect(app, "activate", G_CALLBACK(on_activate), NULL);
    int status = g_application_run(G_APPLICATION(app), argc, argv);
    g_object_unref(app);
    return status;
}
