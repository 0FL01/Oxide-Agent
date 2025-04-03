import time
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler
import subprocess
import sys
import logging 

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

class RestartHandler(FileSystemEventHandler):
    def __init__(self):
        self.process = None
        self.start_bot()

    def start_bot(self):
        if self.process:
            logging.info("Terminating existing bot process...")
            try:
                self.process.terminate()
                self.process.wait(timeout=5) 
                logging.info("Bot process terminated.")
            except subprocess.TimeoutExpired:
                logging.warning("Bot process did not terminate gracefully, killing...")
                self.process.kill()
                self.process.wait()
                logging.info("Bot process killed.")
            except Exception as e:
                 logging.error(f"Error terminating process: {e}")

        logging.info("Starting new bot process: main.py")
        self.process = subprocess.Popen([sys.executable, 'main.py'])
        logging.info(f"Bot process started with PID: {self.process.pid}")

    def on_modified(self, event):
        if event.src_path.endswith('.py') and not event.is_directory:
            logging.info(f"Change detected in Python file: {event.src_path}. Restarting bot...")
            self.start_bot()
        elif not event.is_directory:
             logging.debug(f"Ignoring change in non-Python file: {event.src_path}")


if __name__ == "__main__":
    path = '.' 
    logging.info(f"Starting watchdog observer for path: {path}")
    event_handler = RestartHandler()
    observer = Observer()
    observer.schedule(event_handler, path, recursive=True)
    observer.start()
    logging.info("Watchdog observer started. Monitoring for file changes...")

    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        logging.info("KeyboardInterrupt received. Stopping observer...")
        observer.stop()
        if event_handler.process:
            logging.info("Terminating bot process due to KeyboardInterrupt...")
            event_handler.process.terminate()
            event_handler.process.wait()
            logging.info("Bot process terminated.")
    except Exception as e:
        logging.error(f"An error occurred in the watchdog runner: {e}", exc_info=True)
        observer.stop()
        if event_handler.process:
             event_handler.process.kill()

    observer.join()
    logging.info("Watchdog observer stopped.")