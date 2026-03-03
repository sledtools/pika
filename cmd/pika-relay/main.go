package main

import (
	"context"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"syscall"

	"fiatjaf.com/nostr"
	"fiatjaf.com/nostr/eventstore/lmdb"
	"fiatjaf.com/nostr/khatru"
	"fiatjaf.com/nostr/khatru/blossom"
)

func main() {
	log.SetFlags(log.Ldate | log.Ltime | log.Lshortfile)

	port := envOr("PORT", "3334")
	dataDir := envOr("DATA_DIR", "./data")
	mediaDir := envOr("MEDIA_DIR", "./media")
	// serviceURL is resolved after binding (see below) when PORT=0.
	serviceURLOverride := os.Getenv("SERVICE_URL")

	os.MkdirAll(dataDir, 0755)
	os.MkdirAll(mediaDir, 0755)

	// Bind early so we know the actual port before configuring Blossom.
	ln, err := net.Listen("tcp", ":"+port)
	if err != nil {
		log.Fatalf("failed to listen on :%s: %v", port, err)
	}
	actualPort := ln.Addr().(*net.TCPAddr).Port

	serviceURL := serviceURLOverride
	if serviceURL == "" {
		serviceURL = fmt.Sprintf("http://localhost:%d", actualPort)
	}

	relay := khatru.NewRelay()

	relay.Info.Name = envOr("RELAY_NAME", "pika-relay")
	relay.Info.Description = envOr("RELAY_DESCRIPTION", "Pika relay + Blossom media server")
	relay.Info.Software = "https://github.com/sledtools/pika"
	relay.Info.Version = "0.1.0"

	if pubkey := os.Getenv("RELAY_PUBKEY"); pubkey != "" {
		pk, err := nostr.PubKeyFromHex(pubkey)
		if err == nil {
			relay.Info.PubKey = &pk
		}
	}

	relay.Negentropy = true

	if os.Getenv("PIKA_RELAY_LOG_EVENTS") == "1" {
		log.Printf("event logging enabled (PIKA_RELAY_LOG_EVENTS=1)")
		relay.OnConnect = func(ctx context.Context) {
			log.Printf("[relay/ws] connect ip=%s", khatru.GetIP(ctx))
		}
		relay.OnDisconnect = func(ctx context.Context) {
			log.Printf("[relay/ws] disconnect ip=%s", khatru.GetIP(ctx))
		}
		relay.OnRequest = func(ctx context.Context, filter nostr.Filter) (bool, string) {
			log.Printf(
				"[relay/ws] req ip=%s filter=%s",
				khatru.GetIP(ctx),
				compactFilter(filter),
			)
			return false, ""
		}
		relay.OnEvent = func(ctx context.Context, event nostr.Event) (bool, string) {
			log.Printf(
				"[relay/ws] event_recv ip=%s kind=%d id=%s pubkey=%s tags=%s content=%s",
				khatru.GetIP(ctx),
				event.Kind,
				event.ID.Hex(),
				event.PubKey.Hex(),
				tagSummary(event.Tags),
				contentPreview(event.Content),
			)
			return false, ""
		}
		relay.OnEventSaved = func(_ context.Context, event nostr.Event) {
			log.Printf(
				"[relay/ws] event_saved kind=%d id=%s pubkey=%s tags=%s",
				event.Kind,
				event.ID.Hex(),
				event.PubKey.Hex(),
				tagSummary(event.Tags),
			)
		}
	}

	// Event storage
	db := &lmdb.LMDBBackend{Path: filepath.Join(dataDir, "relay")}
	if err := db.Init(); err != nil {
		log.Fatalf("failed to init relay db: %v", err)
	}
	relay.UseEventstore(db, 500)

	// Blossom
	bdb := &lmdb.LMDBBackend{Path: filepath.Join(dataDir, "blossom")}
	if err := bdb.Init(); err != nil {
		log.Fatalf("failed to init blossom db: %v", err)
	}

	bl := blossom.New(relay, serviceURL)
	bl.Store = blossom.EventStoreBlobIndexWrapper{Store: bdb, ServiceURL: serviceURL}

	bl.StoreBlob = func(ctx context.Context, sha256 string, ext string, body []byte) error {
		path := filepath.Join(mediaDir, sha256)
		return os.WriteFile(path, body, 0644)
	}

	bl.LoadBlob = func(ctx context.Context, sha256 string, ext string) (io.ReadSeeker, *url.URL, error) {
		path := filepath.Join(mediaDir, sha256)
		reader, err := newFileReadSeeker(ctx, path)
		if err != nil {
			return nil, nil, err
		}
		return reader, nil, nil
	}

	bl.DeleteBlob = func(ctx context.Context, sha256 string, ext string) error {
		return os.Remove(filepath.Join(mediaDir, sha256))
	}

	bl.RejectUpload = func(ctx context.Context, auth *nostr.Event, size int, ext string) (bool, string, int) {
		if size > 100*1024*1024 {
			return true, "file too large (100MB max)", 413
		}
		return false, "", 0
	}

	// Health check
	mux := relay.Router()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"status":"ok"}`))
	})

	shutdown := make(chan os.Signal, 1)
	signal.Notify(shutdown, syscall.SIGINT, syscall.SIGTERM)

	srv := &http.Server{Handler: relay}

	go func() {
		log.Printf("pika-relay running on :%d (service_url=%s)", actualPort, serviceURL)
		fmt.Fprintf(os.Stderr, "PIKA_RELAY_PORT=%d\n", actualPort)
		if err := srv.Serve(ln); err != http.ErrServerClosed {
			log.Fatalf("HTTP server error: %v", err)
		}
	}()

	<-shutdown
	log.Println("shutting down...")
	srv.Shutdown(context.Background())
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func compactFilter(filter nostr.Filter) string {
	raw := strings.Join(strings.Fields(filter.String()), " ")
	if len(raw) <= 512 {
		return raw
	}
	return raw[:512] + "...(truncated)"
}

func tagSummary(tags nostr.Tags) string {
	parts := make([]string, 0, 8)
	for _, key := range []string{"h", "e", "p", "d"} {
		if tag := tags.Find(key); len(tag) >= 2 {
			parts = append(parts, key+"="+tag[1])
		}
	}
	if len(parts) == 0 {
		return "-"
	}
	return strings.Join(parts, ",")
}

func contentPreview(content string) string {
	trimmed := strings.TrimSpace(content)
	trimmed = strings.ReplaceAll(trimmed, "\n", "\\n")
	trimmed = strings.ReplaceAll(trimmed, "\r", "")
	if len(trimmed) <= 140 {
		return trimmed
	}
	return trimmed[:140] + "...(truncated)"
}

type fileReadSeeker struct {
	file      *os.File
	reader    *io.SectionReader
	closeOnce sync.Once
}

func newFileReadSeeker(ctx context.Context, path string) (*fileReadSeeker, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	info, err := f.Stat()
	if err != nil {
		_ = f.Close()
		return nil, err
	}

	frs := &fileReadSeeker{
		file:   f,
		reader: io.NewSectionReader(f, 0, info.Size()),
	}

	runtime.SetFinalizer(frs, func(s *fileReadSeeker) {
		s.close()
	})

	go func() {
		<-ctx.Done()
		frs.close()
	}()

	return frs, nil
}

func (f *fileReadSeeker) Read(p []byte) (int, error) {
	n, err := f.reader.Read(p)
	if err == io.EOF {
		f.close()
	}
	return n, err
}

func (f *fileReadSeeker) Seek(offset int64, whence int) (int64, error) {
	return f.reader.Seek(offset, whence)
}

func (f *fileReadSeeker) close() {
	f.closeOnce.Do(func() {
		_ = f.file.Close()
	})
}
